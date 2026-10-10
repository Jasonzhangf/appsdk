//! Host-wide typed state for the v1 global daemon.
//!
//! This module owns the host-wide typed identity, scope and command projection
//! used by the resident daemon.  It stays independent of the legacy
//! project-local [`super::state::State`] data model; the daemon reducer imports
//! these types without creating a second journal or notification store.

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
use std::path::Path;

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

impl RuntimeBindingLedgerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Cold => "cold",
            Self::Missing => "missing",
            Self::RepairRequired => "repair_required",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBindingLedgerRecord {
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub agent_id: AgentId,
    pub runtime_id: RuntimeId,
    pub binding_id: BindingId,
    pub endpoint_generation: u64,
    pub state: RuntimeBindingLedgerState,
    pub probe_state: Option<RuntimeBindingLedgerState>,
    pub reason: Option<String>,
    pub classified_ms: i64,
    pub operation_id: OperationId,
    pub receipt_id: String,
}

impl RuntimeBindingLedgerRecord {
    pub fn key(&self) -> String {
        self.binding_id.as_str().to_owned()
    }

    pub fn validate(&self) -> Result<(), StateError> {
        validate_project_scope(&self.project_scope)?;
        validate_app_scope(&self.app_scope_id)?;
        validate_agent_id(&self.agent_id)?;
        validate_runtime_id(&self.runtime_id)?;
        validate_binding_id(&self.binding_id)?;
        validate_operation_id(&self.operation_id)?;
        if self.endpoint_generation == 0 {
            return Err(StateError::invalid(
                "runtime binding ledger endpoint_generation",
                "must be non-zero",
            ));
        }
        if self.classified_ms < 0 {
            return Err(StateError::invalid(
                "runtime binding ledger classified_ms",
                "must be non-negative",
            ));
        }
        if self.receipt_id.trim().is_empty() || self.receipt_id.len() > 128 {
            return Err(StateError::invalid(
                "runtime binding ledger receipt_id",
                "must be a non-empty short identifier",
            ));
        }
        Ok(())
    }
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
    StaleBinding {
        binding_id: String,
        expected_generation: u64,
        observed_generation: u64,
    },
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
            Self::StaleBinding {
                binding_id,
                expected_generation,
                observed_generation,
            } => write!(
                f,
                "stale runtime binding {binding_id}: expected generation {expected_generation}, observed {observed_generation}"
            ),
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
/// `endpoint_generation` is the reconnect fence: commands must use the
/// current generation exactly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeBinding {
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub agent_id: AgentId,
    pub runtime_id: RuntimeId,
    pub binding_id: BindingId,
    pub endpoint_generation: u64,
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
        endpoint_generation: u64,
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
            endpoint_generation,
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
        endpoint_generation: u64,
        session_id: Option<SessionId>,
        native_thread_id: Option<NativeThreadId>,
    ) -> Result<Self, StateError> {
        let binding = Self {
            project_scope,
            app_scope_id,
            agent_id,
            runtime_id,
            binding_id,
            endpoint_generation,
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
            let tmux_route_matches = self.session_id.as_ref().map(SessionId::as_str)
                == Some(endpoint.tmux_session_id.as_str())
                && self.native_thread_id.as_ref().map(NativeThreadId::as_str)
                    == Some(endpoint.pane_id.as_str());
            let codex_identity_matches = self.session_id.as_ref().map(SessionId::as_str)
                == Some(
                    endpoint
                        .codex_session_id
                        .as_deref()
                        .unwrap_or(&endpoint.tmux_session_id),
                )
                && self.native_thread_id.as_ref().map(NativeThreadId::as_str)
                    == Some(
                        endpoint
                            .codex_thread_id
                            .as_deref()
                            .unwrap_or(&endpoint.pane_id),
                    );
            if !tmux_route_matches && !codex_identity_matches {
                return Err(StateError::invalid(
                    "runtime binding tmux endpoint",
                    "runtime identity must match Codex anchors or the tmux session/pane address",
                ));
            }
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
    pub endpoint_generation: u64,
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
        if rebound_to.endpoint_generation <= old.endpoint_generation {
            return Err(StateError::StaleBinding {
                binding_id: old.binding_id.as_str().to_owned(),
                expected_generation: old.endpoint_generation,
                observed_generation: rebound_to.endpoint_generation,
            });
        }
        Ok(Self {
            project_scope: old.project_scope.clone(),
            app_scope_id: old.app_scope_id.clone(),
            agent_id: old.agent_id.clone(),
            runtime_id: old.runtime_id.clone(),
            binding_id: old.binding_id.clone(),
            endpoint_generation: old.endpoint_generation,
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
    pub endpoint_generation: u64,
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
        endpoint_generation: u64,
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
            endpoint_generation,
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
        endpoint_generation: u64,
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
            endpoint_generation,
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
        if self.grant_generation == 0 {
            self.endpoint_generation
        } else {
            self.grant_generation
        }
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
        if self.grant_generation == 0 && self.endpoint_generation == 0 {
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
/// One durable, immutable intent for an approved master-grant replacement.
/// It is written to the existing resident authority journal before the first
/// grant effect so a started grant owner is provable across restart without
/// replaying that effect. It carries no token or cancellation capability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MasterGrantReplacementIntent {
    pub operation_id: String,
    pub intent_id: String,
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub target_identity: AgentId,
    pub incumbent_grant_id: String,
    pub incumbent_grant_generation: u64,
    pub binding_id: BindingId,
    pub endpoint_generation: u64,
    pub expected_grant_id: String,
    pub expected_grant_generation: u64,
    pub approval_digest: String,
    pub started_at_ms: i64,
}

impl MasterGrantReplacementIntent {
    pub fn validate(&self) -> Result<(), StateError> {
        validate_non_empty_text("grant replacement operation id", &self.operation_id)?;
        validate_non_empty_text("grant replacement intent id", &self.intent_id)?;
        validate_project_scope(&self.project_scope)?;
        validate_app_scope(&self.app_scope_id)?;
        validate_agent_id(&self.target_identity)?;
        validate_binding_id(&self.binding_id)?;
        if self.expected_grant_generation == 0 {
            return Err(StateError::invalid(
                "grant replacement expected generation",
                "must be non-zero",
            ));
        }
        if self.started_at_ms < 0 {
            return Err(StateError::invalid(
                "grant replacement started_at_ms",
                "must be non-negative",
            ));
        }
        Ok(())
    }
}

/// One immutable completion receipt for a grant replacement. It requires a
/// matching durable intent and binds the exact resulting grant resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MasterGrantReplacementReceipt {
    pub operation_id: String,
    pub intent_id: String,
    pub project_scope: ProjectScopeId,
    pub app_scope_id: AppServerId,
    pub grant_id: String,
    pub grant_generation: u64,
    pub binding_id: BindingId,
    pub endpoint_generation: u64,
    pub completed_at_ms: i64,
}

impl MasterGrantReplacementReceipt {
    pub fn validate(&self) -> Result<(), StateError> {
        validate_non_empty_text("grant replacement operation id", &self.operation_id)?;
        validate_non_empty_text("grant replacement intent id", &self.intent_id)?;
        validate_project_scope(&self.project_scope)?;
        validate_app_scope(&self.app_scope_id)?;
        validate_binding_id(&self.binding_id)?;
        if self.grant_generation == 0 {
            return Err(StateError::invalid(
                "grant replacement receipt generation",
                "must be non-zero",
            ));
        }
        if self.completed_at_ms < 0 {
            return Err(StateError::invalid(
                "grant replacement completed_at_ms",
                "must be non-negative",
            ));
        }
        Ok(())
    }
}

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
    pub runtime_binding_ledger: BTreeMap<String, RuntimeBindingLedgerRecord>,
}

impl ProjectState {
    pub fn new(project_scope: ProjectScopeId) -> Result<Self, StateError> {
        validate_project_scope(&project_scope)?;
        Ok(Self {
            project_scope,
            registrations: BTreeMap::new(),
            runtime_bindings: BTreeMap::new(),
            master_grants: BTreeMap::new(),
            runtime_binding_ledger: BTreeMap::new(),
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
            if grant.app_scope_id != binding.app_scope_id
                || grant.agent_id != binding.agent_id
                || grant.endpoint_generation != binding.endpoint_generation
            {
                return Err(StateError::Invariant(format!(
                    "master grant {binding_key} is not bound to the current runtime generation"
                )));
            }
        }

        for (ledger_key, ledger) in &self.runtime_binding_ledger {
            ledger.validate()?;
            if *ledger_key != ledger.key() {
                return Err(StateError::Invariant(format!(
                    "runtime binding ledger key does not match record"
                )));
            }
            let Some(binding) = self.runtime_bindings.get(ledger.binding_id.as_str()) else {
                return Err(StateError::Invariant(format!(
                    "runtime binding ledger references missing binding {}",
                    ledger.binding_id
                )));
            };
            if binding.project_scope != ledger.project_scope
                || binding.app_scope_id != ledger.app_scope_id
                || binding.agent_id != ledger.agent_id
                || binding.runtime_id != ledger.runtime_id
                || binding.binding_id != ledger.binding_id
            {
                return Err(StateError::Invariant(format!(
                    "runtime binding ledger coordinates disagree for {}",
                    ledger.binding_id
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

/// Authoritative target-state evidence for one committed migration operation.
/// The evidence is indexed by `operation_id` in [`GlobalState`], so a receipt
/// can only pass when its coordinates agree with the state-owned commit record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MigrationCommitEvidence {
    pub migration_id: String,
    pub source_project_id: String,
    pub target_epoch: u64,
    pub source_snapshot_digest: String,
    /// Digest of the immutable archive that carries the source stream.  The
    /// receipt owns the authoritative value; this copy is only a projection
    /// input, so a record-only edit cannot satisfy the fence.  `default` keeps
    /// pre-fence journal records deserializable so the gate can refuse them
    /// instead of aborting journal replay.
    #[serde(default)]
    pub archive_digest: String,
    pub operation_id: OperationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<BindingId>,
    pub fencing_token: u64,
    pub committed_revision: u64,
}

impl MigrationCommitEvidence {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        migration_id: impl Into<String>,
        source_project_id: impl Into<String>,
        target_epoch: u64,
        source_snapshot_digest: impl Into<String>,
        operation_id: OperationId,
        binding_id: Option<BindingId>,
        fencing_token: u64,
        committed_revision: u64,
    ) -> Result<Self, StateError> {
        let evidence = Self {
            migration_id: migration_id.into(),
            source_project_id: source_project_id.into(),
            target_epoch,
            source_snapshot_digest: source_snapshot_digest.into(),
            archive_digest: String::new(),
            operation_id,
            binding_id,
            fencing_token,
            committed_revision,
        };
        evidence.validate()?;
        Ok(evidence)
    }

    /// Set the projected archive digest.  The value is only a projection of the
    /// receipt-side truth; the migration gate compares it against the receipt.
    pub fn with_archive_digest(mut self, archive_digest: impl Into<String>) -> Self {
        self.archive_digest = archive_digest.into();
        self
    }

    pub fn validate(&self) -> Result<(), StateError> {
        validate_migration_identifier("migration id", &self.migration_id)?;
        validate_migration_identifier("source project id", &self.source_project_id)?;
        validate_migration_identifier("source snapshot digest", &self.source_snapshot_digest)?;
        validate_operation_id(&self.operation_id)?;
        if let Some(binding_id) = &self.binding_id {
            validate_binding_id(binding_id)?;
        }
        if self.target_epoch == 0 {
            return Err(StateError::invalid(
                "migration target epoch",
                "must be non-zero",
            ));
        }
        if self.fencing_token == 0 {
            return Err(StateError::invalid(
                "migration fencing token",
                "must be non-zero",
            ));
        }
        if self.committed_revision == 0 {
            return Err(StateError::invalid(
                "migration commit revision",
                "must be non-zero",
            ));
        }
        Ok(())
    }
}

/// Receipt proving that exactly one target writer was admitted for a
/// migration attempt.  The receipt is control state: it carries no source
/// payload and does not start or stop a daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MigrationWriterReceipt {
    pub migration_id: String,
    pub source_project_id: String,
    pub source_epoch: Option<u64>,
    pub target_epoch: u64,
    pub source_snapshot_digest: String,
    /// Authoritative digest of the immutable migration archive.  The receipt
    /// owns this value; commit evidence only projects it.  `default` keeps
    /// receipts written before this field existed deserializable.
    #[serde(default)]
    pub archive_digest: String,
    pub writer_id: AgentId,
    pub operation_id: OperationId,
    pub fencing_token: u64,
    pub writer_count: u32,
    pub committed_revision: u64,
}

impl MigrationWriterReceipt {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        migration_id: impl Into<String>,
        source_project_id: impl Into<String>,
        source_epoch: Option<u64>,
        target_epoch: u64,
        source_snapshot_digest: impl Into<String>,
        writer_id: AgentId,
        operation_id: OperationId,
        fencing_token: u64,
        writer_count: u32,
        committed_revision: u64,
    ) -> Result<Self, StateError> {
        let receipt = Self {
            migration_id: migration_id.into(),
            source_project_id: source_project_id.into(),
            source_epoch,
            target_epoch,
            source_snapshot_digest: source_snapshot_digest.into(),
            archive_digest: String::new(),
            writer_id,
            operation_id,
            fencing_token,
            writer_count,
            committed_revision,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    /// Bind the authoritative archive digest this writer committed.
    pub fn with_archive_digest(mut self, archive_digest: impl Into<String>) -> Self {
        self.archive_digest = archive_digest.into();
        self
    }

    pub fn validate(&self) -> Result<(), StateError> {
        validate_migration_identifier("migration id", &self.migration_id)?;
        validate_migration_identifier("source project id", &self.source_project_id)?;
        validate_migration_identifier("source snapshot digest", &self.source_snapshot_digest)?;
        validate_agent_id(&self.writer_id)?;
        validate_operation_id(&self.operation_id)?;
        validate_migration_epoch(self.source_epoch, self.target_epoch)?;
        if self.fencing_token == 0 {
            return Err(StateError::invalid(
                "migration fencing token",
                "must be non-zero",
            ));
        }
        if self.writer_count != 1 {
            return Err(StateError::invalid(
                "migration writer count",
                "exactly one writer is required",
            ));
        }
        if self.committed_revision == 0 {
            return Err(StateError::invalid(
                "migration writer revision",
                "must be non-zero",
            ));
        }
        Ok(())
    }
}

/// Receipt proving that a stable logical identity was rebound to the target
/// epoch's runtime/binding tuple.  A matching runtime receipt is required
/// before the tuple can authorize target state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MigrationIdentityRebindReceipt {
    pub migration_id: String,
    pub source_project_id: String,
    pub project_scope: ProjectScopeId,
    pub source_epoch: Option<u64>,
    pub target_epoch: u64,
    pub source_snapshot_digest: String,
    pub agent_id: AgentId,
    pub app_scope_id: AppServerId,
    pub runtime_id: RuntimeId,
    pub binding_id: BindingId,
    pub endpoint_generation: u64,
    pub operation_id: OperationId,
    pub fencing_token: u64,
    pub committed_revision: u64,
}

impl MigrationIdentityRebindReceipt {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        migration_id: impl Into<String>,
        source_project_id: impl Into<String>,
        project_scope: ProjectScopeId,
        source_epoch: Option<u64>,
        target_epoch: u64,
        source_snapshot_digest: impl Into<String>,
        agent_id: AgentId,
        app_scope_id: AppServerId,
        runtime_id: RuntimeId,
        binding_id: BindingId,
        endpoint_generation: u64,
        operation_id: OperationId,
        fencing_token: u64,
        committed_revision: u64,
    ) -> Result<Self, StateError> {
        let receipt = Self {
            migration_id: migration_id.into(),
            source_project_id: source_project_id.into(),
            project_scope,
            source_epoch,
            target_epoch,
            source_snapshot_digest: source_snapshot_digest.into(),
            agent_id,
            app_scope_id,
            runtime_id,
            binding_id,
            endpoint_generation,
            operation_id,
            fencing_token,
            committed_revision,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<(), StateError> {
        validate_migration_receipt_identity(
            &self.migration_id,
            &self.source_project_id,
            &self.project_scope,
            self.source_epoch,
            self.target_epoch,
            &self.source_snapshot_digest,
            &self.agent_id,
            &self.app_scope_id,
            &self.runtime_id,
            &self.binding_id,
            self.endpoint_generation,
            &self.operation_id,
            self.fencing_token,
            self.committed_revision,
        )
    }
}

/// Receipt proving that the native/runtime endpoint was rebound and committed
/// under the migration writer fence.  Its identity tuple must match an
/// `MigrationIdentityRebindReceipt` exactly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MigrationRuntimeRebindReceipt {
    pub migration_id: String,
    pub source_project_id: String,
    pub project_scope: ProjectScopeId,
    pub source_epoch: Option<u64>,
    pub target_epoch: u64,
    pub source_snapshot_digest: String,
    pub agent_id: AgentId,
    pub app_scope_id: AppServerId,
    pub runtime_id: RuntimeId,
    pub binding_id: BindingId,
    pub endpoint_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_thread_id: Option<NativeThreadId>,
    pub operation_id: OperationId,
    pub fencing_token: u64,
    pub committed_revision: u64,
}

impl MigrationRuntimeRebindReceipt {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        migration_id: impl Into<String>,
        source_project_id: impl Into<String>,
        project_scope: ProjectScopeId,
        source_epoch: Option<u64>,
        target_epoch: u64,
        source_snapshot_digest: impl Into<String>,
        agent_id: AgentId,
        app_scope_id: AppServerId,
        runtime_id: RuntimeId,
        binding_id: BindingId,
        endpoint_generation: u64,
        native_thread_id: Option<NativeThreadId>,
        operation_id: OperationId,
        fencing_token: u64,
        committed_revision: u64,
    ) -> Result<Self, StateError> {
        Self::new_with_session(
            migration_id,
            source_project_id,
            project_scope,
            source_epoch,
            target_epoch,
            source_snapshot_digest,
            agent_id,
            app_scope_id,
            runtime_id,
            binding_id,
            endpoint_generation,
            None,
            native_thread_id,
            operation_id,
            fencing_token,
            committed_revision,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_session(
        migration_id: impl Into<String>,
        source_project_id: impl Into<String>,
        project_scope: ProjectScopeId,
        source_epoch: Option<u64>,
        target_epoch: u64,
        source_snapshot_digest: impl Into<String>,
        agent_id: AgentId,
        app_scope_id: AppServerId,
        runtime_id: RuntimeId,
        binding_id: BindingId,
        endpoint_generation: u64,
        session_id: Option<SessionId>,
        native_thread_id: Option<NativeThreadId>,
        operation_id: OperationId,
        fencing_token: u64,
        committed_revision: u64,
    ) -> Result<Self, StateError> {
        let receipt = Self {
            migration_id: migration_id.into(),
            source_project_id: source_project_id.into(),
            project_scope,
            source_epoch,
            target_epoch,
            source_snapshot_digest: source_snapshot_digest.into(),
            agent_id,
            app_scope_id,
            runtime_id,
            binding_id,
            endpoint_generation,
            session_id,
            native_thread_id,
            operation_id,
            fencing_token,
            committed_revision,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<(), StateError> {
        validate_migration_receipt_identity(
            &self.migration_id,
            &self.source_project_id,
            &self.project_scope,
            self.source_epoch,
            self.target_epoch,
            &self.source_snapshot_digest,
            &self.agent_id,
            &self.app_scope_id,
            &self.runtime_id,
            &self.binding_id,
            self.endpoint_generation,
            &self.operation_id,
            self.fencing_token,
            self.committed_revision,
        )?;
        if let Some(native_thread_id) = &self.native_thread_id {
            validate_native_thread_id(native_thread_id)?;
        }
        if let Some(session_id) = &self.session_id {
            validate_session_id(session_id)?;
        }
        Ok(())
    }
}

include!("global_state_models/migration_receipt_set.rs");
