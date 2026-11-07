//! Host-wide typed state for the v1 global daemon.
//!
//! This module owns the host-wide typed identity, scope and command projection
//! used by the resident daemon.  It stays independent of the legacy
//! project-local [`super::state::State`] data model; the daemon reducer imports
//! these types without creating a second persistence or notification store.

use super::global_state_helpers::*;
pub use crate::identity::{
    AgentId, AppServerId, BindingId, CommandId, NativeThreadId, OperationId, RuntimeId, SessionId,
};
pub use crate::proto::TmuxEndpoint;
pub use crate::scope::{ProjectScopeId, RouteScope};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// A project scope is the canonical, registered project root.  The alias
/// keeps the storage key and the route identity visibly distinct from an
/// execution worktree path.
pub type CanonicalProjectScope = ProjectScopeId;
pub type AppScopeId = AppServerId;

pub const INITIAL_EPOCH: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeBindingLedgerState {
    Live,
    Cold,
    Missing,
    RepairRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBindingLedgerRecord {
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub agent_id: AgentId,
    pub runtime_id: RuntimeId,
    pub binding_id: BindingId,
    pub state: RuntimeBindingLedgerState,
    pub probe_state: Option<RuntimeBindingLedgerState>,
    pub reason: Option<String>,
    pub classified_ms: i64,
    pub operation_id: OperationId,
    pub receipt_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateVersion {
    pub epoch: u64,
    pub sequence: u64,
    pub revision: u64,
}

impl StateVersion {
    pub(crate) fn from_state(state: &GlobalState) -> Self {
        Self {
            epoch: state.epoch,
            sequence: state.sequence,
            revision: state.revision,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    Invalid {
        field: &'static str,
        reason: String,
    },
    ProjectNotRegistered(String),
    RegistrationConflict {
        project_scope: String,
        app_scope_id: String,
    },
    BindingNotFound(String),
    BindingConflict(String),
    MasterGrantRequiresApproval,
    MasterGrantConflict(String),
    MasterGrantBindingMismatch(String),
    CommandIdReuse {
        command_id: String,
        existing_operation: String,
        observed_operation: String,
    },
    OperationIdReuse {
        operation_id: String,
        existing_command: String,
    },
    ReceiptConflict(String),
    CompareAndSwapMismatch {
        expected: u64,
        observed: u64,
    },
    CounterOverflow(&'static str),
    Invariant(String),
}

impl StateError {
    pub(crate) fn invalid(field: &'static str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            field,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { field, reason } => write!(f, "invalid {field}: {reason}"),
            Self::ProjectNotRegistered(scope) => {
                write!(f, "project scope is not registered: {scope}")
            }
            Self::RegistrationConflict {
                project_scope,
                app_scope_id,
            } => write!(
                f,
                "project registration already exists for ({project_scope}, {app_scope_id})"
            ),
            Self::BindingNotFound(binding_id) => write!(f, "runtime binding not found: {binding_id}"),
            Self::BindingConflict(reason) => write!(f, "runtime binding conflict: {reason}"),
            Self::MasterGrantRequiresApproval => {
                f.write_str("master grant requires explicit user approval")
            }
            Self::MasterGrantConflict(reason) => write!(f, "master grant conflict: {reason}"),
            Self::MasterGrantBindingMismatch(reason) => {
                write!(f, "master grant binding mismatch: {reason}")
            }
            Self::CommandIdReuse {
                command_id,
                existing_operation,
                observed_operation,
            } => write!(
                f,
                "command {command_id} belongs to operation {existing_operation}, not {observed_operation}"
            ),
            Self::OperationIdReuse {
                operation_id,
                existing_command,
            } => write!(
                f,
                "operation {operation_id} already belongs to command {existing_command}"
            ),
            Self::ReceiptConflict(reason) => write!(f, "command receipt conflict: {reason}"),
            Self::CompareAndSwapMismatch { expected, observed } => write!(
                f,
                "compare-and-swap revision mismatch: expected {expected}, observed {observed}"
            ),
            Self::CounterOverflow(counter) => write!(f, "{counter} counter overflow"),
            Self::Invariant(reason) => write!(f, "global state invariant failed: {reason}"),
        }
    }
}

impl std::error::Error for StateError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerRole {
    Peer,
    Master,
}

impl Default for PeerRole {
    fn default() -> Self {
        Self::Peer
    }
}

/// A project registration is keyed by its canonical project scope and then
/// by AppServer scope.  A second AppServer for the same project therefore
/// gets a separate registration without replacing the first one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRegistration {
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    #[serde(default)]
    pub registered_at_ms: i64,
}

impl ProjectRegistration {
    pub fn new(
        project_scope: ProjectScopeId,
        app_scope_id: AppServerId,
    ) -> Result<Self, StateError> {
        Self::with_registered_at(project_scope, app_scope_id, 0)
    }

    pub fn with_registered_at(
        project_scope: ProjectScopeId,
        app_scope_id: AppServerId,
        registered_at_ms: i64,
    ) -> Result<Self, StateError> {
        let registration = Self {
            project_scope,
            app_scope_id,
            registered_at_ms,
        };
        registration.validate()?;
        Ok(registration)
    }

    pub fn route_scope(&self) -> RouteScope {
        RouteScope {
            app_scope_id: self.app_scope_id.clone(),
            project_scope_id: self.project_scope.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), StateError> {
        validate_project_scope(&self.project_scope)?;
        validate_app_scope(&self.app_scope_id)
    }
}

/// One current runtime endpoint for one registered project/AppServer scope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeBinding {
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub agent_id: AgentId,
    pub runtime_id: RuntimeId,
    pub binding_id: BindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_thread_id: Option<NativeThreadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_endpoint: Option<TmuxEndpoint>,
}

impl RuntimeBinding {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_scope: ProjectScopeId,
        app_scope_id: AppServerId,
        agent_id: AgentId,
        runtime_id: RuntimeId,
        binding_id: BindingId,
        native_thread_id: Option<NativeThreadId>,
    ) -> Result<Self, StateError> {
        if native_thread_id.is_some() {
            return Err(StateError::invalid(
                "runtime binding",
                "a thread-backed binding requires new_with_session and a verified session id",
            ));
        }
        Self::new_with_session(
            project_scope,
            app_scope_id,
            agent_id,
            runtime_id,
            binding_id,
            None,
            native_thread_id,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_session(
        project_scope: ProjectScopeId,
        app_scope_id: AppServerId,
        agent_id: AgentId,
        runtime_id: RuntimeId,
        binding_id: BindingId,
        session_id: Option<SessionId>,
        native_thread_id: Option<NativeThreadId>,
    ) -> Result<Self, StateError> {
        let binding = Self {
            project_scope,
            app_scope_id,
            agent_id,
            runtime_id,
            binding_id,
            session_id,
            native_thread_id,
            tmux_endpoint: None,
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn route_scope(&self) -> RouteScope {
        RouteScope {
            app_scope_id: self.app_scope_id.clone(),
            project_scope_id: self.project_scope.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), StateError> {
        validate_project_scope(&self.project_scope)?;
        validate_app_scope(&self.app_scope_id)?;
        validate_agent_id(&self.agent_id)?;
        validate_runtime_id(&self.runtime_id)?;
        validate_binding_id(&self.binding_id)?;
        if let Some(session_id) = &self.session_id {
            validate_session_id(session_id)?;
        }
        if let Some(thread_id) = &self.native_thread_id {
            validate_native_thread_id(thread_id)?;
        }
        if let Some(endpoint) = &self.tmux_endpoint {
            validate_tmux_route_endpoint(endpoint)?;
            // The binding identity survives pane movement. The current tmux
            // endpoint is its delivery address, not a second identity proof.
        }
        Ok(())
    }

    pub(crate) fn same_principal(&self, other: &Self) -> bool {
        self.project_scope == other.project_scope
            && self.app_scope_id == other.app_scope_id
            && self.agent_id == other.agent_id
    }
}

/// A retired session/thread address. The replacement is deliberately a
/// binding, not only a route: recovery callers need the exact identity and
/// generation to use without making the old address live again.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeBindingTombstone {
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub agent_id: AgentId,
    pub runtime_id: RuntimeId,
    pub binding_id: BindingId,
    pub session_id: SessionId,
    pub native_thread_id: NativeThreadId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_endpoint: Option<TmuxEndpoint>,
    #[serde(rename = "reboundTo")]
    pub rebound_to: RuntimeBinding,
}

impl RuntimeBindingTombstone {
    pub fn new(old: &RuntimeBinding, rebound_to: &RuntimeBinding) -> Result<Self, StateError> {
        old.validate()?;
        rebound_to.validate()?;
        if old.route_scope() != rebound_to.route_scope()
            || old.agent_id != rebound_to.agent_id
            || old.binding_id != rebound_to.binding_id
        {
            return Err(StateError::BindingConflict(
                "session/thread rebind must preserve project, app scope, agent and binding"
                    .to_owned(),
            ));
        }
        let old_session_id = old.session_id.clone().ok_or_else(|| {
            StateError::invalid("runtime binding tombstone", "old binding has no session id")
        })?;
        let old_native_thread_id = old.native_thread_id.clone().ok_or_else(|| {
            StateError::invalid(
                "runtime binding tombstone",
                "old binding has no native thread id",
            )
        })?;
        let replacement_session_id = rebound_to.session_id.as_ref().ok_or_else(|| {
            StateError::invalid(
                "runtime binding tombstone",
                "replacement binding has no session id",
            )
        })?;
        let replacement_native_thread_id =
            rebound_to.native_thread_id.as_ref().ok_or_else(|| {
                StateError::invalid(
                    "runtime binding tombstone",
                    "replacement binding has no native thread id",
                )
            })?;
        if current_thread_route_address(
            &old_session_id,
            &old_native_thread_id,
            old.tmux_endpoint.as_ref(),
        ) == current_thread_route_address(
            replacement_session_id,
            replacement_native_thread_id,
            rebound_to.tmux_endpoint.as_ref(),
        ) {
            return Err(StateError::BindingConflict(
                "session/thread rebind must change the address".to_owned(),
            ));
        }
        Ok(Self {
            project_scope: old.project_scope.clone(),
            app_scope_id: old.app_scope_id.clone(),
            agent_id: old.agent_id.clone(),
            runtime_id: old.runtime_id.clone(),
            binding_id: old.binding_id.clone(),
            session_id: old_session_id,
            native_thread_id: old_native_thread_id,
            tmux_endpoint: old.tmux_endpoint.clone(),
            rebound_to: rebound_to.clone(),
        })
    }

    pub fn validate(&self) -> Result<(), StateError> {
        validate_project_scope(&self.project_scope)?;
        validate_app_scope(&self.app_scope_id)?;
        validate_agent_id(&self.agent_id)?;
        validate_runtime_id(&self.runtime_id)?;
        validate_binding_id(&self.binding_id)?;
        validate_session_id(&self.session_id)?;
        validate_native_thread_id(&self.native_thread_id)?;
        self.rebound_to.validate()?;
        if self.project_scope != self.rebound_to.project_scope
            || self.app_scope_id != self.rebound_to.app_scope_id
            || self.agent_id != self.rebound_to.agent_id
            || self.binding_id != self.rebound_to.binding_id
        {
            return Err(StateError::Invariant(
                "tombstone replacement does not preserve identity".to_owned(),
            ));
        }
        let replacement_session_id = self.rebound_to.session_id.as_ref().ok_or_else(|| {
            StateError::Invariant("tombstone replacement has no session id".to_owned())
        })?;
        let replacement_native_thread_id =
            self.rebound_to.native_thread_id.as_ref().ok_or_else(|| {
                StateError::Invariant("tombstone replacement has no native thread id".to_owned())
            })?;
        if let Some(endpoint) = self.tmux_endpoint.as_ref() {
            validate_tmux_route_endpoint(endpoint)?;
            let tmux_route_matches = endpoint.tmux_session_id == self.session_id.as_str()
                && endpoint.pane_id == self.native_thread_id.as_str();
            let codex_identity_matches = endpoint
                .codex_session_id
                .as_deref()
                .unwrap_or(&endpoint.tmux_session_id)
                == self.session_id.as_str()
                && endpoint
                    .codex_thread_id
                    .as_deref()
                    .unwrap_or(&endpoint.pane_id)
                    == self.native_thread_id.as_str();
            if !tmux_route_matches && !codex_identity_matches {
                return Err(StateError::Invariant(
                    "tombstone runtime identity does not match its old tmux/Codex address"
                        .to_owned(),
                ));
            }
        }
        if current_thread_route_address(
            &self.session_id,
            &self.native_thread_id,
            self.tmux_endpoint.as_ref(),
        ) == current_thread_route_address(
            replacement_session_id,
            replacement_native_thread_id,
            self.rebound_to.tmux_endpoint.as_ref(),
        ) {
            return Err(StateError::Invariant(
                "tombstone replacement has the same address".to_owned(),
            ));
        }
        Ok(())
    }
}

/// A master capability is an explicit grant bound to one live runtime
/// generation.  A registration has no role field: absence of this record is
/// the durable default `Peer` role.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MasterGrant {
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub agent_id: AgentId,
    pub boundary: String,
    pub granted_by: String,
    pub approval: String,
    #[serde(default)]
    pub grant_id: String,
    #[serde(default)]
    pub grant_generation: u64,
    pub binding_id: BindingId,
    pub granted_at_ms: i64,
}

impl MasterGrant {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_scope: ProjectScopeId,
        app_scope_id: AppServerId,
        agent_id: AgentId,
        boundary: impl Into<String>,
        granted_by: impl Into<String>,
        approval: impl Into<String>,
        binding_id: BindingId,
        granted_at_ms: i64,
    ) -> Result<Self, StateError> {
        let grant = Self {
            project_scope,
            app_scope_id,
            agent_id,
            boundary: boundary.into(),
            granted_by: granted_by.into(),
            approval: approval.into(),
            grant_id: binding_id.as_str().to_owned(),
            grant_generation: 1,
            binding_id,
            granted_at_ms,
        };
        grant.validate()?;
        Ok(grant)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_resource(
        project_scope: ProjectScopeId,
        app_scope_id: AppServerId,
        agent_id: AgentId,
        boundary: impl Into<String>,
        granted_by: impl Into<String>,
        approval: impl Into<String>,
        grant_id: impl Into<String>,
        grant_generation: u64,
        binding_id: BindingId,
        granted_at_ms: i64,
    ) -> Result<Self, StateError> {
        let grant = Self {
            project_scope,
            app_scope_id,
            agent_id,
            boundary: boundary.into(),
            granted_by: granted_by.into(),
            approval: approval.into(),
            grant_id: grant_id.into(),
            grant_generation,
            binding_id,
            granted_at_ms,
        };
        grant.validate()?;
        Ok(grant)
    }

    pub fn resource_id(&self) -> &str {
        if self.grant_id.is_empty() {
            self.binding_id.as_str()
        } else {
            self.grant_id.as_str()
        }
    }

    pub fn resource_generation(&self) -> u64 {
        self.grant_generation
    }

    pub fn validate(&self) -> Result<(), StateError> {
        validate_project_scope(&self.project_scope)?;
        validate_app_scope(&self.app_scope_id)?;
        validate_agent_id(&self.agent_id)?;
        validate_binding_id(&self.binding_id)?;
        if self.grant_id.chars().any(char::is_control) {
            return Err(StateError::invalid(
                "master grant id",
                "must not contain control characters",
            ));
        }
        if self.grant_generation == 0 {
            return Err(StateError::invalid(
                "master grant generation",
                "must be non-zero",
            ));
        }
        validate_non_empty_text("master grant boundary", &self.boundary)?;
        validate_non_empty_text("master grant actor", &self.granted_by)?;
        if self.approval.trim().is_empty() {
            return Err(StateError::MasterGrantRequiresApproval);
        }
        if self.approval.chars().any(char::is_control) {
            return Err(StateError::invalid(
                "master grant approval",
                "must not contain control characters",
            ));
        }
        Ok(())
    }
}

/// Project-local state owned by this model.  It contains only registrations,
/// runtime bindings and capability grants.  Tasks, messages and legacy
/// notification records are intentionally absent so this module cannot become
/// a second copy of `server::state::State`.
/// Project-local state owned by this model.  It contains only registrations,
/// runtime bindings and capability grants.  Tasks, messages and legacy
/// notification records are intentionally absent so this module cannot become
/// a second copy of `server::state::State`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectState {
    pub project_scope: ProjectScopeId,
    #[serde(default)]
    pub registrations: BTreeMap<String, ProjectRegistration>,
    #[serde(default)]
    pub runtime_bindings: BTreeMap<String, RuntimeBinding>,
    #[serde(default)]
    pub master_grants: BTreeMap<String, MasterGrant>,
    #[serde(default)]
    pub bootstrap_policies: BTreeMap<String, BootstrapPolicy>,
}

impl ProjectState {
    pub fn new(project_scope: ProjectScopeId) -> Result<Self, StateError> {
        validate_project_scope(&project_scope)?;
        Ok(Self {
            project_scope,
            registrations: BTreeMap::new(),
            runtime_bindings: BTreeMap::new(),
            master_grants: BTreeMap::new(),
            bootstrap_policies: BTreeMap::new(),
        })
    }

    pub fn validate(&self) -> Result<(), StateError> {
        validate_project_scope(&self.project_scope)?;

        for (app_scope_key, registration) in &self.registrations {
            registration.validate()?;
            if registration.project_scope != self.project_scope {
                return Err(StateError::Invariant(format!(
                    "registration {} belongs to {}, expected {}",
                    app_scope_key,
                    registration.project_scope.as_str(),
                    self.project_scope.as_str()
                )));
            }
            if app_scope_key != registration.app_scope_id.as_str() {
                return Err(StateError::Invariant(format!(
                    "registration key {app_scope_key} does not match app scope {}",
                    registration.app_scope_id
                )));
            }
        }

        let mut runtime_ids = BTreeSet::new();
        for (binding_key, binding) in &self.runtime_bindings {
            binding.validate()?;
            if binding.project_scope != self.project_scope {
                return Err(StateError::Invariant(format!(
                    "binding {binding_key} belongs to another project scope"
                )));
            }
            if binding_key != binding.binding_id.as_str() {
                return Err(StateError::Invariant(format!(
                    "binding key {binding_key} does not match binding {}",
                    binding.binding_id
                )));
            }
            if !self
                .registrations
                .contains_key(binding.app_scope_id.as_str())
            {
                return Err(StateError::Invariant(format!(
                    "binding {binding_key} has no app scope registration"
                )));
            }
            if !runtime_ids.insert(binding.runtime_id.as_str().to_owned()) {
                return Err(StateError::Invariant(format!(
                    "runtime {} is bound more than once",
                    binding.runtime_id
                )));
            }
        }

        for (binding_key, grant) in &self.master_grants {
            grant.validate()?;
            if grant.project_scope != self.project_scope {
                return Err(StateError::Invariant(format!(
                    "master grant {binding_key} belongs to another project scope"
                )));
            }
            if binding_key != grant.binding_id.as_str() {
                return Err(StateError::Invariant(format!(
                    "master grant key {binding_key} does not match binding {}",
                    grant.binding_id
                )));
            }
            let Some(binding) = self.runtime_bindings.get(binding_key) else {
                return Err(StateError::Invariant(format!(
                    "master grant {binding_key} has no runtime binding"
                )));
            };
            if grant.app_scope_id != binding.app_scope_id || grant.agent_id != binding.agent_id {
                return Err(StateError::Invariant(format!(
                    "master grant {binding_key} is not bound to the current runtime generation"
                )));
            }
        }

        Ok(())
    }

    pub fn lookup_registration(&self, app_scope_id: &AppServerId) -> Option<&ProjectRegistration> {
        self.registrations.get(app_scope_id.as_str())
    }

    pub fn lookup_binding(&self, binding_id: &BindingId) -> Option<&RuntimeBinding> {
        self.runtime_bindings.get(binding_id.as_str())
    }

    pub fn lookup_master_grant(&self, binding_id: &BindingId) -> Option<&MasterGrant> {
        self.master_grants.get(binding_id.as_str())
    }
}

/// One explicit project/app authorization, consumed by the first registration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BootstrapPolicy {
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub authorization: String,
    pub authorized_operation: String,
    pub consumption: Option<BootstrapConsumption>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BootstrapConsumption {
    pub command_id: String,
    pub binding_id: BindingId,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandReceipt {
    pub command_id: CommandId,
    pub operation_id: OperationId,
    pub epoch: u64,
    pub sequence: u64,
    pub revision: u64,
    #[serde(default)]
    pub outcome: Value,
}

impl CommandReceipt {
    pub fn validate(&self) -> Result<(), StateError> {
        validate_command_id(&self.command_id)?;
        validate_operation_id(&self.operation_id)?;
        if self.epoch == 0 {
            return Err(StateError::invalid(
                "command receipt epoch",
                "must be non-zero",
            ));
        }
        if self.sequence == 0 {
            return Err(StateError::invalid(
                "command receipt sequence",
                "must be non-zero",
            ));
        }
        if self.revision == 0 {
            return Err(StateError::invalid(
                "command receipt revision",
                "must be non-zero",
            ));
        }
        Ok(())
    }
}

/// Current host-wide state. Project and AppServer data remain scoped; command
/// IDs are host-wide so retries cannot be rebound across projects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlobalState {
    pub epoch: u64,
    pub sequence: u64,
    pub revision: u64,
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectState>,
    #[serde(default, with = "crate::server::current_state::rows")]
    pub current_thread_routes: BTreeMap<(String, String), RuntimeBinding>,
    /// Address index for bindings without a session key. This index locates
    /// endpoints; only identity-map.json selects an identity.
    #[serde(default, with = "crate::server::current_state::rows")]
    pub legacy_thread_routes: BTreeMap<(String, String, String), RuntimeBinding>,
    #[serde(default)]
    pub current_thread_route_tombstones: BTreeMap<String, RuntimeBindingTombstone>,
    #[serde(default)]
    pub command_receipts: BTreeMap<String, CommandReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LedgerScanReceipt {
    pub scan_id: String,
    pub scanned_ms: i64,
    pub classified: u32,
    pub transitioned: u32,
    pub unchanged: u32,
    pub blocked: u32,
    pub mailbox_messages_unchanged: bool,
}
