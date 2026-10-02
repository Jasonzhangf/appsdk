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
        TransportKind::Dsh => {
            // The dsh endpoint is the gateway control socket. It gets its own
            // `unix://` judgement rather than sharing the App Server one: the
            // scheme requirement is the same, but relaxing it for the new kind
            // is exactly the hole this branch exists to close.
            if !endpoint.starts_with("unix://") {
                anyhow::bail!("selected dsh transport endpoint is not unix://");
            }
            transport
                .namespace
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("selected dsh transport has no gateway runtime id")
                })?;
            if transport.tmux_endpoint.is_some() {
                anyhow::bail!("selected dsh transport must not carry a tmux pane binding");
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
    /// The anchor belongs to another project scope. `anchor_peers` holds every
    /// record that matched this anchor, so a caller can refuse to retire the
    /// anchor while any duplicate may still own it.
    CrossProject {
        chosen: Identity,
        anchor_peers: Vec<Identity>,
    },
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
        Some(AnchorResolution::CrossProject { .. }) => anyhow::bail!(
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
    // Every record that matched any anchor of a worker, deduplicated by
    // worker_id. One identity can match several anchors (its Codex session and
    // its thread), so a later anchor must add to this set instead of replacing
    // the duplicates an earlier anchor already contributed.
    let mut anchor_groups = BTreeMap::<String, BTreeMap<String, Identity>>::new();
    let expected_scope = scope
        .route_scope(AppServerId::new(CLI_APP_SERVER_ID)?)?
        .project_scope_id;
    for (anchor, identities) in matches {
        let group = identities.into_values().collect::<Vec<_>>();
        if group.len() > 1 {
            let chosen = choose_anchor_peer(
                host_paths,
                candidate,
                &expected_scope,
                &anchor,
                group.clone(),
                persisted_peer_liveness,
            )?;
            let peers = anchor_groups.entry(chosen.worker_id.clone()).or_default();
            for identity in group {
                peers.insert(identity.worker_id.clone(), identity);
            }
            matched_workers.insert(chosen.worker_id.clone(), chosen);
        } else {
            for identity in group {
                anchor_groups
                    .entry(identity.worker_id.clone())
                    .or_default()
                    .insert(identity.worker_id.clone(), identity.clone());
                matched_workers.insert(identity.worker_id.clone(), identity);
            }
        }
    }
    match matched_workers.len() {
        0 => Ok(None),
        1 => {
            let (worker_id, identity) = matched_workers.into_iter().next().unwrap();
            let anchor_peers = anchor_groups
                .remove(&worker_id)
                .map(|peers| peers.into_values().collect::<Vec<_>>())
                .unwrap_or_else(|| vec![identity.clone()]);
            Ok(Some(
                if identity.project_scope.as_ref() == Some(&expected_scope) {
                    AnchorResolution::CurrentScope(identity)
                } else {
                    AnchorResolution::CrossProject {
                        chosen: identity,
                        anchor_peers,
                    }
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
        // An explicit `--worker` names the durable identity to load, so it is
        // decided before anchor resolution: an anchor ambiguity or scope
        // mismatch is exactly the case the override exists to resolve.
        if explicit_worker.is_none() {
            identity_by_current_anchors_same_scope_at(host_paths, scope, tmux_candidate.as_ref())?
        } else {
            None
        };
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
#[derive(Debug)]
enum ScopeRebindOutcome {
    /// One durable identity matches a current tmux/Codex anchor.
    Adopted(Identity),
    /// No durable record left to protect: first registration for this project,
    /// or every stale record was provably dead and has been archived.
    NoCandidate,
    /// A *live* durable peer already claims this exact anchor. Adopting another
    /// record would collide with a reachable peer, so the caller must fail
    /// closed and require the explicit `--worker` override.
    Unproven(String),
}

/// Whether a persisted peer can still be reached. Only `Dead` authorizes
/// retiring the record and must never be treated as a live conflict. A probe
/// that merely failed is `Unknown`, and a cold-but-resumable record is `Cold`:
/// "cannot prove it is gone" and "not currently loaded" are both distinct from
/// "it is gone" and from "it is live". Only a *live* overlap is a hard
/// conflict; a `Cold`/`Unknown` record is not live, so it never blocks recovery
/// and is instead a normal drift/restart candidate the current pane adopts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeerLiveness {
    /// Actively reachable: a live tmux pane or a loaded AppServer thread.
    Live,
    /// Provably gone: a missing pane, an errored thread, or an explicit
    /// not-found/rolled-out probe result. Only this authorizes retirement.
    Dead,
    /// Not currently live but resumable: an AppServer thread that is not
    /// loaded. It is preserved (never archived) and is not a live conflict.
    Cold,
    /// Liveness could not be established (probe error, malformed address, or
    /// a missing transport). It is not live, so it never blocks recovery; like
    /// `Cold` it is a normal drift/restart candidate the current pane adopts.
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
        TransportKind::Dsh => {
            // A dsh peer is re-anchored by challenging the gateway, not by
            // comparing a pane. Only an explicit "the gateway does not know
            // this agent" retires the record: a gateway that is down, slow or
            // answering garbage says nothing about the agent, so it stays
            // `Unknown` and never authorizes retirement.
            let (Some(endpoint), Some(runtime_id), Some(agent_id)) = (
                transport.endpoint.as_deref(),
                transport.namespace.as_deref(),
                transport.thread_id.as_deref(),
            ) else {
                return PeerLiveness::Unknown;
            };
            match crate::client::adapters::dsh::probe(endpoint, runtime_id, agent_id) {
                crate::client::adapters::dsh::PeerPresence::Live => PeerLiveness::Live,
                crate::client::adapters::dsh::PeerPresence::Absent => PeerLiveness::Dead,
                crate::client::adapters::dsh::PeerPresence::Unknown => PeerLiveness::Unknown,
            }
        }
    }
}

/// Only an explicitly dead signal retires the record. A `notLoaded` thread is
/// cold, not gone: the AppServer contract can resume it through `turn/start`,
/// so it is classified as non-live and stays recoverable instead of being
/// archived.
fn classify_thread_status(raw: &serde_json::Value) -> PeerLiveness {
    match raw
        .pointer("/thread/status/type")
        .and_then(serde_json::Value::as_str)
    {
        Some("systemError") => PeerLiveness::Dead,
        // A successful read that reports `notLoaded` is a definitive "not
        // currently live, but resumable" answer, not an unproven one.
        Some("notLoaded") => PeerLiveness::Cold,
        Some(_) => PeerLiveness::Live,
        None => PeerLiveness::Unknown,
    }
}

/// Only an explicitly dead signal retires the record. A malformed App Server
/// response (a missing field, a decode failure) is unproven, not dead: the
/// endpoint answered, so the thread may still be live and must not authorize a
/// credential retirement. `notLoaded` is reported by `classify_thread_status`,
/// not here.
fn classify_probe_error(detail: &str) -> PeerLiveness {
    let lowered = detail.to_ascii_lowercase();
    if lowered.contains("no rollout")
        || lowered.contains("thread not found")
        || lowered.contains("tmux_pane_missing")
    {
        PeerLiveness::Dead
    } else {
        PeerLiveness::Unknown
    }
}

/// Durable registration recency for one persisted identity. The identity file
/// is rewritten atomically on every registration, so its last write time is a
/// globally ordered "when was this record last refreshed" signal that stays
/// valid across different worker/binding ids. A per-binding
/// `endpoint_generation` restarts at 1 for a new binding and is therefore not
/// a global order.
fn identity_recency_at(host_paths: &HostPaths, worker_id: &str) -> std::time::SystemTime {
    let path = host_paths
        .state_root()
        .join("identities")
        .join(worker_id)
        .join("identity.json");
    std::fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(std::time::UNIX_EPOCH)
}

/// Deterministic preference between two records that both match the current
/// anchor and are not live. Prefer the record this pane derives its own id
/// from (`codex-<pane>`), then the most recently registered record, then the
/// lowest worker id so the outcome is stable even on an exact timestamp tie.
fn non_live_candidate_order(
    host_paths: &HostPaths,
    candidate: Option<&crate::proto::TmuxCandidate>,
    left: &Identity,
    right: &Identity,
) -> std::cmp::Ordering {
    let derived = candidate.map(|candidate| format!("codex-{}", candidate.endpoint.pane_id));
    let left_own = derived.as_deref() == Some(left.worker_id.as_str());
    let right_own = derived.as_deref() == Some(right.worker_id.as_str());
    right_own
        .cmp(&left_own)
        .then_with(|| {
            identity_recency_at(host_paths, &right.worker_id)
                .cmp(&identity_recency_at(host_paths, &left.worker_id))
        })
        .then_with(|| left.worker_id.cmp(&right.worker_id))
}

/// Resolve one anchor that several persisted records claim.
///
/// Project scope is applied first: a record registered under another project
/// can never be the current project's peer for the same anchor, so a foreign
/// duplicate can neither shadow nor be retired in place of a current-scope
/// match. Every member here matches this exact anchor, so only a *live* member
/// is a hard conflict: two reachable peers cannot share one anchor, so the
/// explicit `--worker` override must decide. Dead, cold, and unproven members
/// are all non-live, so the deterministic non-live order adopts this peer's own
/// drifted registration instead of blocking recovery.
fn choose_anchor_peer(
    host_paths: &HostPaths,
    candidate: Option<&crate::proto::TmuxCandidate>,
    expected_scope: &crate::scope::ProjectScopeId,
    anchor: &str,
    mut candidates: Vec<Identity>,
    liveness: impl Fn(&Identity) -> PeerLiveness,
) -> anyhow::Result<Identity> {
    // A reachable peer that claims this anchor is a hard conflict regardless of
    // which project scope it registered under. Scope preference must never hide
    // a live owner (including a foreign one) behind a non-live current-scope
    // duplicate.
    if candidates
        .iter()
        .any(|identity| matches!(liveness(identity), PeerLiveness::Live))
    {
        anyhow::bail!(
            "IDENTITY_RESTORE_AMBIGUOUS: {anchor} matches multiple live peers; pass --worker <worker_id> to explicitly select one"
        );
    }
    let in_scope = candidates
        .iter()
        .filter(|identity| identity.project_scope.as_ref() == Some(expected_scope))
        .cloned()
        .collect::<Vec<_>>();
    if !in_scope.is_empty() {
        candidates = in_scope;
        // A provably dead record is only chosen when every same-scope record
        // that claims this anchor is provably dead. If any current-scope record
        // is still cold/unproven it is the surviving recovery candidate, so a
        // newer Dead record must never be revived by durable recency.
        let (dead, surviving): (Vec<_>, Vec<_>) = candidates
            .into_iter()
            .partition(|identity| matches!(liveness(identity), PeerLiveness::Dead));
        candidates = if surviving.is_empty() { dead } else { surviving };
    }
    candidates.sort_by(|left, right| non_live_candidate_order(host_paths, candidate, left, right));
    Ok(candidates.remove(0))
}

/// Whether a persisted peer's durable anchor overlaps the current pane or the
/// Codex session/thread being recovered. Only a peer that is *live* AND
/// overlaps this anchor can block recovery; a live peer on an unrelated pane
/// or thread must not gate a fresh pane or an anchor-drift recovery.
///
/// A tmux peer is addressed by its pane, but an App Server peer is addressed by
/// its Codex session/thread and may keep a tmux recovery anchor. The pane is
/// therefore only an anchor for a tmux transport, or when the current process
/// carries no Codex IDs and the pane is its only anchor; otherwise a shared
/// pane would hide a distinct live App Server thread.
///
/// The candidate's own anchors are the only reliable anchors here. Do not fall
/// back to ambient `CODEX_*` values: a candidate is an existing peer or a
/// recovered address that must be compared on its own fields.
fn identity_anchor_conflicts_with_candidate(
    identity: &Identity,
    candidate: Option<&crate::proto::TmuxCandidate>,
) -> bool {
    let current_session = candidate
        .and_then(|candidate| candidate.endpoint.codex_session_id.clone());
    let current_thread = candidate
        .and_then(|candidate| candidate.endpoint.codex_thread_id.clone());
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

    let pane_is_anchor = transport.is_some_and(|transport| transport.kind == TransportKind::Tmux)
        || (current_session.is_none() && current_thread.is_none());
    if pane_is_anchor {
        if let Some(candidate) = candidate {
            if let Some(endpoint) = transport.and_then(|transport| transport.tmux_endpoint.as_ref())
            {
                if crate::client::adapters::tmux::same_pane_route(endpoint, &candidate.endpoint) {
                    return true;
                }
            }
        }
    }

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
/// transport liveness) and single-sink. Normal session/thread/pane drift
/// adopts the unique durable candidate even when current anchors no longer
/// match, and provably dead records are archived. Only a *live* record that
/// overlaps the current anchor is a hard conflict: two reachable peers cannot
/// share one anchor. Cold/unproven records are not live, so they never block:
/// whether overlapping or not, they are normal drift/restart candidates the
/// current pane adopts deterministically. The explicit user override
/// (selected_worker) is resolved first and directly by path, before any anchor
/// or liveness work, so the named durable identity is always recoverable on
/// request without probing the very records it exists to bypass.
fn identity_for_scope_rebind_at(
    host_paths: &HostPaths,
    scope: &Scope,
    selected_worker: Option<&str>,
) -> anyhow::Result<ScopeRebindOutcome> {
    // The explicit override names the durable identity to recover, so it is
    // resolved first, directly by path, before any anchor or liveness work: it
    // exists precisely to bypass an ambiguous anchor or a stale/silent peer, so
    // probing the very records it must bypass could delay the recovery by the
    // probe timeout or fail it outright. A name with no record is
    // `NoCandidate`; the caller decides whether that may mint.
    if let Some(selected) = selected_worker {
        return Ok(
            match read_identity(&identity_path_at(host_paths, selected)?)? {
                Some(identity) => ScopeRebindOutcome::Adopted(identity),
                None => ScopeRebindOutcome::NoCandidate,
            },
        );
    }
    let project_scope = scope
        .route_scope(AppServerId::new(CLI_APP_SERVER_ID)?)?
        .project_scope_id;
    // Adopting a persisted peer only re-anchors it, so a caller must hold a
    // current anchor to ask for one. Without any pane, Codex session or thread
    // address the caller stays unauthenticated and only an explicit `--worker`
    // may name a durable identity.
    if std::env::var_os("TMUX_PANE").is_none()
        && std::env::var_os("CODEX_SESSION_ID").is_none()
        && std::env::var_os("CODEX_THREAD_ID").is_none()
    {
        return Ok(ScopeRebindOutcome::Unproven(
            "no current anchor; pass --worker to explicitly recover a durable identity".into(),
        ));
    }
    let candidate = if std::env::var_os("TMUX_PANE").is_some() {
        Some(crate::client::adapters::tmux::candidate_from_env().map_err(anyhow::Error::msg)?)
    } else {
        None
    };
    if let Some(identity) =
        identity_by_current_anchors_same_scope_at(host_paths, scope, candidate.as_ref())?
    {
        return Ok(ScopeRebindOutcome::Adopted(identity));
    }
    let identities_root = host_paths.state_root().join("identities");
    if !identities_root.is_dir() {
        return Ok(ScopeRebindOutcome::NoCandidate);
    }
    // Every same-scope record lands in exactly one bucket: provably dead,
    // live conflict (a reachable peer that claims this exact anchor), or a
    // non-live recovery candidate (cold/unproven anywhere, or a live peer on an
    // unrelated anchor that is neither ours nor blocking).
    let mut dead = Vec::new();
    let mut live_conflict = Vec::new();
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
        let overlaps = identity_anchor_conflicts_with_candidate(&identity, candidate.as_ref());
        match persisted_peer_liveness(&identity) {
            PeerLiveness::Dead => dead.push(identity),
            // Only a *live* record that claims the current pane/session/thread
            // is a hard conflict: two reachable peers cannot share one anchor,
            // so recovery must fail closed and let the explicit `--worker`
            // override resolve it.
            PeerLiveness::Live if overlaps => live_conflict.push(identity),
            // A live record on an unrelated anchor is a different, healthy
            // peer: it neither blocks nor is adopted.
            PeerLiveness::Live => {}
            // Cold/Unknown records are not live, so they never conflict. They
            // are the normal drift/restart candidates: the current pane adopts
            // the best one and registration refreshes its stale anchor.
            PeerLiveness::Cold | PeerLiveness::Unknown => recoverable.push(identity),
        }
    }
    // Provably dead same-scope records never block recovery.
    archive_dead_peers(host_paths, &dead)?;

    // A live peer that claims the current anchor is a real conflict, not drift:
    // it is reachable and must not be silently superseded. Fail closed so the
    // explicit `--worker` override decides.
    if !live_conflict.is_empty() {
        let conflict_ids = live_conflict
            .iter()
            .map(|identity| identity.worker_id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(ScopeRebindOutcome::Unproven(format!(
            "live peers overlap the current anchor ({conflict_ids}); pass --worker to explicitly override before recovery"
        )));
    }

    // No live peer claims this anchor, so this is normal drift or a restart.
    // Adopt the deterministic best non-live record (a record on the current
    // anchor first, then the pane-derived id, then durable recency) and let
    // registration refresh its stale anchor. With no candidate this is a first
    // registration for the project.
    if recoverable.is_empty() {
        return Ok(ScopeRebindOutcome::NoCandidate);
    }
    recoverable.sort_by(|left, right| {
        let left_overlap = identity_anchor_conflicts_with_candidate(left, candidate.as_ref());
        let right_overlap = identity_anchor_conflicts_with_candidate(right, candidate.as_ref());
        right_overlap
            .cmp(&left_overlap)
            .then_with(|| non_live_candidate_order(host_paths, candidate.as_ref(), left, right))
    });
    Ok(ScopeRebindOutcome::Adopted(recoverable.remove(0)))
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
    if let Some(named) = explicit_worker.as_deref() {
        // An explicit `--worker` names the durable identity to recover, so it is
        // decided before anchor resolution. Anchor ambiguity, a cross-project
        // record, and a live duplicate are exactly the cases the override exists
        // to resolve, and a named identity must never need a liveness probe to
        // be recovered. Only the unnamed path resolves the current anchors.
        match identity_for_scope_rebind_at(host_paths, scope, Some(named))? {
            ScopeRebindOutcome::Adopted(identity) => return Ok(identity),
            ScopeRebindOutcome::Unproven(detail) => {
                anyhow::bail!("IDENTITY_REBIND_UNPROVEN: {detail}")
            }
            ScopeRebindOutcome::NoCandidate => {
                // The name resolves to no durable record, so this is a typo or a
                // stale name, not a recovery. Minting would create a second
                // owner for an anchor that already belongs to a peer, so fail
                // closed exactly like the unnamed conflict path.
                match identity_by_current_anchors_at(
                    host_paths,
                    scope,
                    candidate.as_ref().ok().copied(),
                )? {
                    Some(AnchorResolution::CurrentScope(identity)) => anyhow::bail!(
                        "IDENTITY_RESTORE_CONFLICT: --worker {named} names no durable identity and the current anchor already belongs to {}",
                        identity.worker_id
                    ),
                    Some(AnchorResolution::CrossProject { chosen, .. }) => anyhow::bail!(
                        "IDENTITY_RESTORE_CROSS_PROJECT: --worker {named} names no durable identity and the current anchor belongs to another project ({})",
                        chosen.worker_id
                    ),
                    None => {}
                }
            }
        }
    } else {
        match identity_by_current_anchors_at(host_paths, scope, candidate.as_ref().ok().copied())? {
            Some(AnchorResolution::CurrentScope(identity)) => return Ok(identity),
            Some(AnchorResolution::CrossProject {
                chosen,
                anchor_peers,
            }) => {
                if allow_scope_rebind {
                    // The same pane/thread previously registered in another
                    // project. A pane/thread can only belong to one live peer,
                    // so the anchor is retired and the current project mints a
                    // fresh peer only when every duplicate that claims it is
                    // provably dead. A live, cold, or unproven duplicate may
                    // still be the live owner of this anchor, so it stays
                    // fail-closed and needs the explicit --worker override
                    // instead of being archived on scope mismatch alone.
                    if anchor_peers
                        .iter()
                        .all(|peer| matches!(persisted_peer_liveness(peer), PeerLiveness::Dead))
                    {
                        archive_dead_peers(host_paths, &anchor_peers)?;
                        retired_cross_project = true;
                    } else {
                        anyhow::bail!(
                            "IDENTITY_RESTORE_CROSS_PROJECT: anchor peer {} belongs to another project and not every duplicate claiming the anchor is provably dead; pass --worker to explicitly recover it",
                            chosen.worker_id
                        );
                    }
                } else {
                    anyhow::bail!(
                        "IDENTITY_RESTORE_CROSS_PROJECT: a unique tmux/Codex anchor belongs to another project"
                    );
                }
            }
            None => {}
        }
    }

    if allow_scope_rebind && !retired_cross_project && explicit_worker.is_none() {
        match identity_for_scope_rebind_at(host_paths, scope, None)? {
            ScopeRebindOutcome::Adopted(identity) => return Ok(identity),
            ScopeRebindOutcome::NoCandidate => {}
            ScopeRebindOutcome::Unproven(detail) => {
                anyhow::bail!("IDENTITY_REBIND_UNPROVEN: {detail}")
            }
        }
    }

    // The anchor set is exactly what the error above advertises: a tmux pane, an
    // existing App Server worker in this scope, or an explicit worker id. The
    // explicit-id case was missing from this guard, so `--worker`/`COLLAB_WORKER`
    // alone still failed with COLLAB_IDENTITY_ANCHOR_MISSING even though it named
    // a valid identity. A dsh peer depends on this: the gateway registers it as
    // an independent peer under its own worker id and has no pane or App Server
    // thread to anchor to.
    if tmux_candidate.is_none() && appserver_worker.is_none() && explicit_worker.is_none() {
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
