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

/// An archived credential is only a candidate. Registration must still prove
/// its token against the daemon's authoritative worker record.
fn archived_pane_identity_at(
    host_paths: &HostPaths,
    scope: &Scope,
    candidate: &crate::proto::TmuxCandidate,
    route: &crate::proto::RouteResolution,
) -> anyhow::Result<Option<Identity>> {
    let expected_scope = scope
        .route_scope(route.app_scope_id.clone())?
        .project_scope_id;
    if route.project_scope != expected_scope {
        anyhow::bail!("IDENTITY_RESTORE_CROSS_PROJECT: pane route belongs to another project");
    }
    let root = host_paths.state_root().join("archives");
    if !root.is_dir() {
        return Ok(None);
    }
    let mut selected: Option<Identity> = None;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir()
            || !entry.file_name().to_string_lossy().starts_with("identities-retired-")
        {
            continue;
        }
        let path = entry.path().join(route.agent_id.as_str()).join("identity.json");
        let Some(identity) = read_identity(&path)? else {
            continue;
        };
        let matches = identity.worker_id == route.agent_id.as_str()
            && identity.project_scope.as_ref() == Some(&route.project_scope)
            && identity.runtime.as_ref().is_some_and(|runtime| {
                let current_generation = runtime.endpoint_generation == route.endpoint_generation
                    && runtime.session_id.as_ref() == Some(&route.session_id)
                    && runtime.native_thread_id.as_ref() == Some(&route.native_thread_id);
                let committed_recovery_predecessor = runtime
                    .endpoint_generation
                    .checked_add(1)
                    == Some(route.endpoint_generation);
                runtime.agent_id == route.agent_id
                    && runtime.binding_id == route.binding_id
                    && runtime.appserver_id == route.app_scope_id
                    && (current_generation || committed_recovery_predecessor)
            })
            && identity.transport.as_ref().is_some_and(|transport| {
                transport.kind == TransportKind::Tmux
                    && transport.tmux_endpoint.as_ref().is_some_and(|endpoint| {
                        crate::client::adapters::tmux::same_pane_route(
                            endpoint,
                            &candidate.endpoint,
                        )
                    })
            });
        if !matches {
            continue;
        }
        let selected_generation = selected
            .as_ref()
            .and_then(|previous| previous.runtime.as_ref())
            .map(|runtime| runtime.endpoint_generation)
            .unwrap_or(0);
        let observed_generation = identity.runtime.as_ref().unwrap().endpoint_generation;
        if observed_generation < selected_generation {
            continue;
        }
        if observed_generation == selected_generation && selected.as_ref().is_some_and(|previous| {
            previous.token != identity.token || previous.runtime != identity.runtime
        }) {
            anyhow::bail!(
                "IDENTITY_RESTORE_AMBIGUOUS: archived pane credentials disagree for registered worker"
            );
        }
        selected = Some(identity);
    }
    Ok(selected)
}

fn recover_archived_pane_at(
    host_paths: &HostPaths,
    scope: &Scope,
    worker_id: &str,
    candidate: &crate::proto::TmuxCandidate,
) -> anyhow::Result<Option<Identity>> {
    if candidate.endpoint.codex_session_id.is_none()
        || candidate.endpoint.codex_thread_id.is_none()
    {
        return Ok(None);
    }
    let archives = host_paths.state_root().join("archives");
    if !archives.is_dir() {
        return Ok(None);
    }
    let scope_id = scope
        .route_scope(AppServerId::new(CLI_APP_SERVER_ID)?)?
        .project_scope_id;
    let mut credential: Option<Identity> = None;
    for entry in std::fs::read_dir(&archives)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir()
            || !entry.file_name().to_string_lossy().starts_with("identities-retired-")
        {
            continue;
        }
        let Some(identity) = read_identity(&entry.path().join(worker_id).join("identity.json"))?
        else {
            continue;
        };
        let matches = identity.project_scope.as_ref() == Some(&scope_id)
            && identity.transport.as_ref().is_some_and(|transport| {
                transport.kind == TransportKind::Tmux
                    && transport.tmux_endpoint.as_ref().is_some_and(|endpoint| {
                        crate::client::adapters::tmux::same_pane_route(
                            endpoint,
                            &candidate.endpoint,
                        )
                    })
            });
        if matches {
            let generation = identity.runtime.as_ref().map(|runtime| runtime.endpoint_generation).unwrap_or(0);
            let previous_generation = credential.as_ref().and_then(|previous| previous.runtime.as_ref())
                .map(|runtime| runtime.endpoint_generation).unwrap_or(0);
            if generation == previous_generation && credential.as_ref().is_some_and(|previous|
                previous.token != identity.token || previous.runtime != identity.runtime) {
                anyhow::bail!("IDENTITY_RESTORE_AMBIGUOUS: archived pane credentials disagree for registered worker");
            }
            if credential.is_none() || generation > previous_generation {
                credential = Some(identity);
            }
        }
    }
    let Some(credential) = credential else {
        return Ok(None);
    };
    let mut pane_only = candidate.endpoint.clone();
    pane_only.codex_session_id = None;
    pane_only.codex_thread_id = None;
    let route: Result<crate::proto::RouteResolution, _> = crate::client::call(
        &scope.sock_path(),
        &crate::proto::Req::RouteResolve {
            tmux_endpoint: pane_only,
        },
    );
    match route {
        Ok(route) => {
            route.validate()?;
            if route.agent_id.as_str() != worker_id {
                anyhow::bail!("IDENTITY_RESTORE_CONFLICT: pane route belongs to another worker");
            }
            archived_pane_identity_at(host_paths, scope, candidate, &route)
        }
        Err(error) if error.to_string().contains("conflicts with its runtime binding") => {
            let route: crate::proto::RouteResolution = crate::client::call(
                &scope.sock_path(),
                &crate::proto::Req::RouteResolvePaneRecovery {
                    tmux_endpoint: candidate.endpoint.clone(),
                    worker_id: worker_id.to_owned(),
                    token: credential.token,
                },
            )?;
            route.validate()?;
            archived_pane_identity_at(host_paths, scope, candidate, &route)
        }
        Err(error) if error.to_string().contains("ROUTE_RESOLVE_NOT_FOUND") => Ok(None),
        Err(error) => Err(error),
    }
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
    identity_by_current_anchors_same_scope_at(host_paths, scope, Some(candidate))
}

/// What a current tmux/Codex anchor uniquely resolved to. The calling path
/// decides whether a cross-project peer may be retired so `collab context`
/// can re-register the same pane/thread under the current project instead of
/// stranding the agent in a hard-fail recovery loop.
enum AnchorResolution {
    /// The anchor belongs to the current project scope.
    CurrentScope(Identity),
    /// The anchor uniquely belongs to another project scope.
    CrossProject(Identity),
}

/// Fail-closed wrapper for scope resolution, init, and explicit recovery:
/// a cross-project match is an error on these paths.
fn identity_by_current_anchors_same_scope_at(
    host_paths: &HostPaths,
    scope: &Scope,
    candidate: Option<&crate::proto::TmuxCandidate>,
) -> anyhow::Result<Option<Identity>> {
    match identity_by_current_anchors_at(host_paths, scope, candidate)? {
        Some(AnchorResolution::CurrentScope(identity)) => Ok(Some(identity)),
        Some(AnchorResolution::CrossProject(_)) => anyhow::bail!(
            "IDENTITY_RESTORE_CROSS_PROJECT: a unique tmux/Codex anchor belongs to another project"
        ),
        None => Ok(None),
    }
}

fn identity_by_current_anchors_at(
    host_paths: &HostPaths,
    scope: &Scope,
    candidate: Option<&crate::proto::TmuxCandidate>,
) -> anyhow::Result<Option<AnchorResolution>> {
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
            Ok(Some(
                if identity.project_scope.as_ref() == Some(&expected_scope) {
                    AnchorResolution::CurrentScope(identity)
                } else {
                    AnchorResolution::CrossProject(identity)
                },
            ))
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
    load_or_create_full(&HostPaths::resolve()?, scope, worker_id, true, false)
}

/// Identity entry point used only by `collab context`.
///
/// `context` is the single bootstrap command an agent is expected to run, so it
/// may auto-register the current pane/thread instead of requiring a human or
/// agent to decide which persisted peer is stale and to pass `--worker-id`.
pub fn load_or_create_for_context(
    scope: &Scope,
    worker_id: Option<String>,
) -> anyhow::Result<Identity> {
    load_or_create_full(&HostPaths::resolve()?, scope, worker_id, true, true)
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
        identity_by_current_anchors_same_scope_at(host_paths, scope, tmux_candidate.as_ref())?;
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
pub fn load_or_create_for_init(
    scope: &Scope,
    worker_id: Option<String>,
) -> anyhow::Result<Identity> {
    load_or_create_for_init_at(&HostPaths::resolve()?, scope, worker_id)
}

fn load_or_create_for_init_at(
    host_paths: &HostPaths,
    scope: &Scope,
    worker_id: Option<String>,
) -> anyhow::Result<Identity> {
    load_or_create_resolved_full_at(host_paths, scope, worker_id, true, false)
}

pub(crate) fn load_or_create_full(
    host_paths: &HostPaths,
    scope: &Scope,
    worker_id: Option<String>,
    allow_scope_rebind: bool,
    allow_fresh_registration: bool,
) -> anyhow::Result<Identity> {
    load_or_create_resolved_full_at(
        host_paths,
        scope,
        worker_id,
        allow_scope_rebind,
        allow_fresh_registration,
    )
}

/// Why identity loading may not mint a brand-new peer for this project.
enum ScopeRebindOutcome {
    /// One durable identity matches a current tmux/Codex anchor.
    Adopted(Identity),
    /// No durable record left to protect: first registration for this project,
    /// or every stale record was provably dead and has been archived.
    NoCandidate,
    /// A durable record exists but its state cannot be established. Minting
    /// here would silently orphan it, so the caller must fail closed.
    Unproven(String),
}

/// Whether a persisted peer can still be reached. Only `Dead` authorizes
/// retiring the record: a probe that merely failed is `Unknown` and must keep
/// blocking, because "cannot prove it is gone" is not "it is gone".
enum PeerLiveness {
    Live,
    Dead,
    Unknown,
}

fn persisted_peer_liveness(identity: &Identity) -> PeerLiveness {
    let Some(transport) = identity.transport.as_ref() else {
        // No transport cannot be proven dead. Fail closed: a record with no
        // re-anchor is still protected unless an endpoint probe proves it is
        // gone.
        return PeerLiveness::Unknown;
    };
    match transport.kind {
        TransportKind::Tmux => {
            let Some(endpoint) = transport.tmux_endpoint.as_ref() else {
                return PeerLiveness::Unknown;
            };
            match crate::client::adapters::tmux::probe(endpoint) {
                Ok(crate::client::adapters::tmux::PanePresence::Present) => PeerLiveness::Live,
                Ok(crate::client::adapters::tmux::PanePresence::Missing) => PeerLiveness::Dead,
                Ok(crate::client::adapters::tmux::PanePresence::Unknown) | Err(_) => {
                    PeerLiveness::Unknown
                }
            }
        }
        TransportKind::AppServer => {
            let Some(thread_id) = transport.thread_id.as_deref() else {
                return PeerLiveness::Unknown;
            };
            match crate::client::adapters::codex_app_server::read_thread_status(
                transport, thread_id,
            ) {
                Ok(raw) => classify_thread_status(&raw),
                Err(error) => classify_probe_error(&error.to_string()),
            }
        }
    }
}

/// Only an explicitly dead signal retires the record. A `notLoaded` thread is
/// cold, not gone: the AppServer contract can resume it through `turn/start`,
/// so it must keep blocking rebind instead of being archived.
fn classify_thread_status(raw: &serde_json::Value) -> PeerLiveness {
    match raw
        .pointer("/thread/status/type")
        .and_then(serde_json::Value::as_str)
    {
        Some("systemError") => PeerLiveness::Dead,
        Some("notLoaded") => PeerLiveness::Unknown,
        Some(_) => PeerLiveness::Live,
        None => PeerLiveness::Unknown,
    }
}

fn classify_probe_error(detail: &str) -> PeerLiveness {
    let lowered = detail.to_ascii_lowercase();
    if lowered.contains("not found")
        || lowered.contains("no rollout")
        || lowered.contains("missing")
        || lowered.contains("gone")
    {
        PeerLiveness::Dead
    } else {
        PeerLiveness::Unknown
    }
}

/// Whether a persisted peer's durable anchor overlaps the current pane or the
/// Codex session/thread being recovered. Only a peer that is *live* AND
/// overlaps this anchor can block recovery; a live peer on an unrelated pane
/// or thread must not gate a fresh pane or an anchor-drift recovery.
fn identity_anchor_conflicts_with_candidate(
    identity: &Identity,
    candidate: Option<&crate::proto::TmuxCandidate>,
) -> bool {
    if let Some(candidate) = candidate {
        if let Some(endpoint) = identity
            .transport
            .as_ref()
            .and_then(|transport| transport.tmux_endpoint.as_ref())
        {
            if crate::client::adapters::tmux::same_pane_route(endpoint, &candidate.endpoint) {
                return true;
            }
        }
    }

    let current_session = candidate
        .and_then(|candidate| candidate.endpoint.codex_session_id.clone())
        .or_else(|| std::env::var("CODEX_SESSION_ID").ok());
    let current_thread = candidate
        .and_then(|candidate| candidate.endpoint.codex_thread_id.clone())
        .or_else(|| std::env::var("CODEX_THREAD_ID").ok());
    let runtime = identity.runtime.as_ref();
    let transport = identity.transport.as_ref();
    let persisted_session = transport
        .and_then(|transport| transport.session_id.as_deref())
        .or_else(|| {
            runtime
                .and_then(|runtime| runtime.session_id.as_ref())
                .map(|session| session.as_str())
        });
    let persisted_thread = transport
        .and_then(|transport| transport.thread_id.as_deref())
        .or_else(|| {
            runtime
                .and_then(|runtime| runtime.native_thread_id.as_ref())
                .map(|thread| thread.as_str())
        });
    if let (Some(left), Some(right)) = (persisted_session, current_session.as_deref()) {
        if left == right {
            return true;
        }
    }
    if let (Some(left), Some(right)) = (persisted_thread, current_thread.as_deref()) {
        if left == right {
            return true;
        }
    }
    false
}

/// Move provably dead peers out of the live identity set so a new pane can
/// register. The bytes are archived, never deleted, and only peers whose
/// endpoint is *proven* gone are retired.
fn archive_dead_peers(host_paths: &HostPaths, dead: &[Identity]) -> anyhow::Result<()> {
    if dead.is_empty() {
        return Ok(());
    }
    let archive_root = host_paths
        .state_root()
        .join("archives")
        .join(format!("identities-retired-{}", now_ms()));
    std::fs::create_dir_all(&archive_root)?;
    for identity in dead {
        validate_id(&identity.worker_id)?;
        let source = host_paths
            .state_root()
            .join("identities")
            .join(&identity.worker_id);
        let destination = archive_root.join(&identity.worker_id);
        std::fs::rename(&source, &destination).with_context(|| {
            format!(
                "IDENTITY_RETIRE_FAILED: cannot archive stale peer {} at {}",
                identity.worker_id,
                source.display()
            )
        })?;
    }
    Ok(())
}

/// Resolve one persisted same-scope identity to the current process.
///
/// Recovery is single-source (persisted identities + current project scope +
/// daemon liveness) and single-sink. Normal session/thread/pane drift adopts
/// the unique durable candidate even when current anchors no longer match.
/// Only a LIVE conflict denies recovery; provably dead records are archived.
/// The explicit user override (selected_worker) may supersede a live
/// conflict (and any ambiguous unknown set) before the selected durable
/// identity is adopted, or clear the field for a fresh selected registration.
fn identity_for_scope_rebind_at(
    host_paths: &HostPaths,
    scope: &Scope,
    selected_worker: Option<&str>,
) -> anyhow::Result<ScopeRebindOutcome> {
    let project_scope = scope
        .route_scope(AppServerId::new(CLI_APP_SERVER_ID)?)?
        .project_scope_id;
    let candidate = if std::env::var_os("TMUX_PANE").is_some() {
        Some(crate::client::adapters::tmux::candidate_from_env().map_err(anyhow::Error::msg)?)
    } else {
        None
    };
    if selected_worker.is_none() {
        if let Some(identity) =
            identity_by_current_anchors_same_scope_at(host_paths, scope, candidate.as_ref())?
        {
            return Ok(ScopeRebindOutcome::Adopted(identity));
        }
    }
    let identities_root = host_paths.state_root().join("identities");
    if !identities_root.is_dir() {
        return Ok(ScopeRebindOutcome::NoCandidate);
    }
    let mut dead = Vec::new();
    let mut live_peers = Vec::new();
    let mut live_conflicts = Vec::new();
    let mut recoverable = Vec::new();
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
        let conflict = identity_anchor_conflicts_with_candidate(&identity, candidate.as_ref());
        match persisted_peer_liveness(&identity) {
            PeerLiveness::Dead => dead.push(identity),
            PeerLiveness::Live => {
                live_peers.push(identity.clone());
                if conflict {
                    live_conflicts.push(identity);
                }
            }
            PeerLiveness::Unknown => recoverable.push(identity),
        }
    }
    if let Some(selected) = selected_worker {
        // Explicit user override: retire live conflicts and ambiguous unknown
        // records, then recover the named durable identity (or return
        // NoCandidate so the caller may mint it fresh).
        let selected_persisted = live_peers
            .iter()
            .chain(recoverable.iter())
            .chain(dead.iter())
            .find(|identity| identity.worker_id == selected)
            .cloned();
        return Ok(match selected_persisted {
            Some(identity) => ScopeRebindOutcome::Adopted(identity),
            None => ScopeRebindOutcome::NoCandidate,
        });
    }

    // Provably dead same-scope records never block recovery.
    archive_dead_peers(host_paths, &dead)?;

    if !live_conflicts.is_empty() {
        let conflict_ids = live_conflicts
            .iter()
            .map(|identity| identity.worker_id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(ScopeRebindOutcome::Unproven(format!(
            "persisted live peers conflict with the current anchor ({conflict_ids}); pass --worker to explicitly override the live conflict before recovery"
        )));
    }

    match recoverable.len() {
        0 => Ok(ScopeRebindOutcome::NoCandidate),
        1 => Ok(ScopeRebindOutcome::Adopted(recoverable.remove(0))),
        _ => Ok(ScopeRebindOutcome::Unproven(format!(
            "persisted same-scope peers are ambiguous ({}); pass --worker to explicitly select one before recovery",
            recoverable
                .iter()
                .map(|identity| identity.worker_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn load_or_create_resolved_at(
    host_paths: &HostPaths,
    scope: &Scope,
    worker_id: Option<String>,
    allow_scope_rebind: bool,
) -> anyhow::Result<Identity> {
    load_or_create_resolved_full_at(host_paths, scope, worker_id, allow_scope_rebind, false)
}

fn load_or_create_resolved_full_at(
    host_paths: &HostPaths,
    scope: &Scope,
    worker_id: Option<String>,
    allow_scope_rebind: bool,
    allow_fresh_registration: bool,
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
    let project_scope = scope
        .route_scope(AppServerId::new(CLI_APP_SERVER_ID)?)?
        .project_scope_id;
    let appserver_worker = current_appserver_worker_id(&project_scope)?;
    let candidate = tmux_candidate.as_ref().ok_or_else(|| {
        anyhow::anyhow!("COLLAB_IDENTITY_ANCHOR_MISSING: identity requires a tmux pane, a valid App Server endpoint, or an explicit worker_id")
    });
    let mut retired_cross_project = false;
    let anchored_identity = match identity_by_current_anchors_at(
        host_paths,
        scope,
        candidate.as_ref().ok().copied(),
    )? {
        Some(AnchorResolution::CurrentScope(identity)) => Some(identity),
        Some(AnchorResolution::CrossProject(identity)) => {
            if allow_scope_rebind {
                // The same pane/thread previously registered in another
                // project. The pane can only belong to one live Collab peer,
                // so archive the stale cross-project record and let the
                // current project mint a fresh peer instead of leaving the
                // agent stuck in a manual recovery loop.
                archive_dead_peers(host_paths, std::slice::from_ref(&identity))?;
                retired_cross_project = true;
                None
            } else {
                anyhow::bail!(
                    "IDENTITY_RESTORE_CROSS_PROJECT: a unique tmux/Codex anchor belongs to another project"
                );
            }
        }
        None => None,
    };
    if let Some(identity) = anchored_identity {
        if explicit_worker.as_ref().is_some_and(|worker| worker.as_str() != identity.worker_id) {
            anyhow::bail!(
                "IDENTITY_RESTORE_CONFLICT: explicit worker {:?} conflicts with the current tmux/Codex anchors for {}",
                explicit_worker,
                identity.worker_id
            );
        }
        return Ok(identity);
    }

    if allow_scope_rebind && !retired_cross_project {
        match identity_for_scope_rebind_at(host_paths, scope, explicit_worker.as_deref())? {
            ScopeRebindOutcome::Adopted(identity) => return Ok(identity),
            ScopeRebindOutcome::NoCandidate => {}
            ScopeRebindOutcome::Unproven(detail) => {
                anyhow::bail!("IDENTITY_REBIND_UNPROVEN: {detail}")
            }
        }
    }

    if tmux_candidate.is_none() && appserver_worker.is_none() {
        candidate?;
    }
    let worker_id = explicit_worker
        .clone()
        .or_else(|| {
            tmux_candidate
                .as_ref()
                .map(|candidate| format!("codex-{}", candidate.endpoint.pane_id))
        })
        .or(appserver_worker)
        .ok_or_else(|| {
            anyhow::anyhow!("collab identity requires TMUX_PANE or an explicit worker id")
        })?;
    if let Some(ident) = read_identity(&identity_path_at(host_paths, &worker_id)?)? {
        if allow_fresh_registration && ident.runtime.is_none() {
            if let Some(candidate) = tmux_candidate.as_ref() {
                if let Some(archived) =
                    recover_archived_pane_at(host_paths, scope, &worker_id, candidate)?
                {
                    return Ok(archived);
                }
            }
        }
        return Ok(ident);
    }
    if allow_fresh_registration {
        if let Some(candidate) = tmux_candidate.as_ref() {
            if let Some(archived) =
                recover_archived_pane_at(host_paths, scope, &worker_id, candidate)?
            {
                return Ok(archived);
            }
        }
    }
    let ident = Identity {
        worker_id,
        token: hex(16),
        project_scope: Some(project_scope),
        runtime: None,
        transport: None,
    };
    write_identity(&identity_path_at(host_paths, &ident.worker_id)?, &ident)?;
    Ok(ident)
}

/// Stable, filesystem-safe peer identity for a new native App Server thread
/// when the project has no persisted peer identity yet. In an existing
/// project, callers must explicitly supply `worker_id` when no prior runtime
/// anchor matches so identity recovery remains fail-closed.
/// Prefer an already-known Collab worker for this native Codex thread.
///
/// A Tmux-hosted Codex TUI and a Codex-native peer can share the same
/// CODEX_THREAD_ID while living under different workers. Use the exact
/// session/thread match first; otherwise preserve the legacy fallback used
/// for first registration and explicit worker selection.
fn current_appserver_worker_id(
    project_scope: &crate::scope::ProjectScopeId,
) -> anyhow::Result<Option<String>> {
    if std::env::var_os("CODEX_THREAD_ID").is_none()
        || std::env::var_os("CODEX_SESSION_ID").is_none()
    {
        return Ok(None);
    }
    let Ok(Some(candidate)) = crate::client::adapters::candidate_from_env() else {
        return Ok(None);
    };
    let session_id = SessionId::new(candidate.session_id)?;
    if let Ok(host_paths) = HostPaths::resolve() {
        let mut matches = identities_by_runtime_key_at(
            &host_paths,
            session_id.as_str(),
            candidate.thread_id.as_str(),
        )?;
        matches.retain(|identity| identity.project_scope.as_ref() == Some(project_scope));
        if matches.len() == 1 {
            return Ok(Some(matches.remove(0).worker_id));
        }
    }
    let mut encoded = String::with_capacity(candidate.thread_id.len() * 2);
    for byte in candidate.thread_id.as_bytes() {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    let worker_id = format!("codex-thread-{encoded}");
    validate_id(&worker_id)?;
    Ok(Some(worker_id))
}

#[cfg(test)]
#[path = "identity_tests.rs"]
mod tests;
