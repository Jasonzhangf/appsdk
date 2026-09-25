//! Host-wide typed state for the v1 global daemon.
//!
//! This module owns the host-wide typed identity, scope and command projection
//! used by the resident daemon.  It stays independent of the legacy
//! project-local [`super::state::State`] data model; the daemon reducer imports
//! these types without creating a second journal or notification store.

use crate::identity::{
    AgentId, AppServerId, BindingId, CommandId, NativeThreadId, OperationId, RuntimeId, SessionId,
};
use crate::proto::TmuxEndpoint;
use crate::scope::{ProjectScopeId, RouteScope};
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
pub struct StateVersion {
    pub epoch: u64,
    pub sequence: u64,
    pub revision: u64,
}

impl StateVersion {
    fn from_state(state: &GlobalState) -> Self {
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
    fn invalid(field: &'static str, reason: impl Into<String>) -> Self {
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
            binding_id,
            endpoint_generation,
            granted_at_ms,
        };
        grant.validate()?;
        Ok(grant)
    }

    pub fn validate(&self) -> Result<(), StateError> {
        validate_project_scope(&self.project_scope)?;
        validate_app_scope(&self.app_scope_id)?;
        validate_agent_id(&self.agent_id)?;
        validate_binding_id(&self.binding_id)?;
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectState {
    pub project_scope: ProjectScopeId,
    #[serde(default)]
    pub registrations: BTreeMap<String, ProjectRegistration>,
    #[serde(default)]
    pub runtime_bindings: BTreeMap<String, RuntimeBinding>,
    #[serde(default)]
    pub master_grants: BTreeMap<String, MasterGrant>,
}

impl ProjectState {
    pub fn new(project_scope: ProjectScopeId) -> Result<Self, StateError> {
        validate_project_scope(&project_scope)?;
        Ok(Self {
            project_scope,
            registrations: BTreeMap::new(),
            runtime_bindings: BTreeMap::new(),
            master_grants: BTreeMap::new(),
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

/// The receipt graph required by the migration apply gate.  The optional
/// writer is intentional: a missing writer remains a typed validation error,
/// rather than being represented by an invented default writer.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MigrationReceiptSet {
    pub writer: Option<MigrationWriterReceipt>,
    #[serde(default)]
    pub identity_rebinds: Vec<MigrationIdentityRebindReceipt>,
    #[serde(default)]
    pub runtime_rebinds: Vec<MigrationRuntimeRebindReceipt>,
}

impl MigrationReceiptSet {
    pub fn validate(&self) -> Result<(), StateError> {
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| StateError::invalid("migration writer receipt", "is required"))?;
        self.validate_for(
            &writer.migration_id,
            &writer.source_project_id,
            &writer.source_snapshot_digest,
            writer.target_epoch,
        )
    }

    pub fn validate_for(
        &self,
        migration_id: &str,
        source_project_id: &str,
        source_snapshot_digest: &str,
        target_epoch: u64,
    ) -> Result<(), StateError> {
        validate_migration_identifier("migration id", migration_id)?;
        validate_migration_identifier("source project id", source_project_id)?;
        validate_migration_identifier("source snapshot digest", source_snapshot_digest)?;
        if target_epoch == 0 {
            return Err(StateError::invalid(
                "migration target epoch",
                "must be non-zero",
            ));
        }
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| StateError::invalid("migration writer receipt", "is required"))?;
        writer.validate()?;
        validate_migration_context(
            "writer",
            &writer.migration_id,
            &writer.source_project_id,
            &writer.source_snapshot_digest,
            writer.target_epoch,
            migration_id,
            source_project_id,
            source_snapshot_digest,
            target_epoch,
        )?;

        if self.identity_rebinds.len() != self.runtime_rebinds.len() {
            return Err(StateError::Invariant(format!(
                "migration identity/runtime receipt count mismatch: {} != {}",
                self.identity_rebinds.len(),
                self.runtime_rebinds.len()
            )));
        }

        let mut operations = BTreeSet::new();
        if !operations.insert(writer.operation_id.as_str().to_owned()) {
            return Err(StateError::Invariant(format!(
                "migration operation {} is recorded more than once",
                writer.operation_id
            )));
        }

        for receipt in &self.identity_rebinds {
            receipt.validate()?;
            validate_migration_context(
                "identity rebind",
                &receipt.migration_id,
                &receipt.source_project_id,
                &receipt.source_snapshot_digest,
                receipt.target_epoch,
                migration_id,
                source_project_id,
                source_snapshot_digest,
                target_epoch,
            )?;
            if receipt.fencing_token != writer.fencing_token {
                return Err(StateError::Invariant(format!(
                    "identity rebind {} uses a different fencing token",
                    receipt.operation_id
                )));
            }
            if receipt.source_epoch != writer.source_epoch {
                return Err(StateError::Invariant(format!(
                    "identity rebind {} uses a different source epoch",
                    receipt.operation_id
                )));
            }
            if !operations.insert(receipt.operation_id.as_str().to_owned()) {
                return Err(StateError::Invariant(format!(
                    "migration operation {} is recorded more than once",
                    receipt.operation_id
                )));
            }
        }

        for receipt in &self.runtime_rebinds {
            receipt.validate()?;
            validate_migration_context(
                "runtime rebind",
                &receipt.migration_id,
                &receipt.source_project_id,
                &receipt.source_snapshot_digest,
                receipt.target_epoch,
                migration_id,
                source_project_id,
                source_snapshot_digest,
                target_epoch,
            )?;
            if receipt.fencing_token != writer.fencing_token {
                return Err(StateError::Invariant(format!(
                    "runtime rebind {} uses a different fencing token",
                    receipt.operation_id
                )));
            }
            if receipt.source_epoch != writer.source_epoch {
                return Err(StateError::Invariant(format!(
                    "runtime rebind {} uses a different source epoch",
                    receipt.operation_id
                )));
            }
            if !operations.insert(receipt.operation_id.as_str().to_owned()) {
                return Err(StateError::Invariant(format!(
                    "migration operation {} is recorded more than once",
                    receipt.operation_id
                )));
            }
        }

        for identity in &self.identity_rebinds {
            let matches = self.runtime_rebinds.iter().filter(|runtime| {
                runtime.project_scope == identity.project_scope
                    && runtime.app_scope_id == identity.app_scope_id
                    && runtime.agent_id == identity.agent_id
                    && runtime.runtime_id == identity.runtime_id
                    && runtime.binding_id == identity.binding_id
                    && runtime.endpoint_generation == identity.endpoint_generation
                    && runtime.fencing_token == identity.fencing_token
            });
            if matches.count() != 1 {
                return Err(StateError::Invariant(format!(
                    "identity rebind {} has no unique matching runtime receipt",
                    identity.operation_id
                )));
            }
        }
        for runtime in &self.runtime_rebinds {
            let matches = self.identity_rebinds.iter().filter(|identity| {
                identity.project_scope == runtime.project_scope
                    && identity.app_scope_id == runtime.app_scope_id
                    && identity.agent_id == runtime.agent_id
                    && identity.runtime_id == runtime.runtime_id
                    && identity.binding_id == runtime.binding_id
                    && identity.endpoint_generation == runtime.endpoint_generation
                    && identity.fencing_token == runtime.fencing_token
            });
            if matches.count() != 1 {
                return Err(StateError::Invariant(format!(
                    "runtime rebind {} has no unique matching identity receipt",
                    runtime.operation_id
                )));
            }
        }
        Ok(())
    }
}

/// One host-wide reducer state.  Project state is nested under a canonical
/// project-scope key; AppServer registrations are nested under each project.
/// Command IDs are host-wide so retries cannot be rebound across projects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlobalState {
    pub epoch: u64,
    pub sequence: u64,
    pub revision: u64,
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectState>,
    #[serde(default)]
    pub current_thread_routes: BTreeMap<(String, String), RuntimeBinding>,
    /// Read-only compatibility index for durable bindings that carry a native
    /// App Server thread but no session id.  These records predate the strict
    /// dual key, so replay must keep them resolvable instead of aborting the
    /// host journal.  A thread may legitimately appear under several binding
    /// ids (a disposable project ran once on the same peer thread), so the
    /// index stores one binding per `(app_scope, thread, binding_id)` and keeps
    /// only the highest generation for each.  The key order is app scope,
    /// native thread, then binding id.
    #[serde(default)]
    pub legacy_thread_routes: BTreeMap<(String, String, String), RuntimeBinding>,
    #[serde(default)]
    pub current_thread_route_tombstones: BTreeMap<String, RuntimeBindingTombstone>,
    #[serde(default)]
    pub command_receipts: BTreeMap<String, CommandReceipt>,
    #[serde(default)]
    pub migration_commit_evidence: BTreeMap<String, MigrationCommitEvidence>,
}

impl Default for GlobalState {
    fn default() -> Self {
        Self::new(INITIAL_EPOCH).expect("the fixed initial epoch is valid")
    }
}

impl GlobalState {
    pub fn new(epoch: u64) -> Result<Self, StateError> {
        if epoch == 0 {
            return Err(StateError::invalid("epoch", "must be non-zero"));
        }
        Ok(Self {
            epoch,
            sequence: 0,
            revision: 0,
            projects: BTreeMap::new(),
            current_thread_routes: BTreeMap::new(),
            legacy_thread_routes: BTreeMap::new(),
            current_thread_route_tombstones: BTreeMap::new(),
            command_receipts: BTreeMap::new(),
            migration_commit_evidence: BTreeMap::new(),
        })
    }

    pub fn version(&self) -> StateVersion {
        StateVersion::from_state(self)
    }

    /// The daemon journal owns the host version when this projection is
    /// nested in `server::state::State`.  Keep these counters synchronized
    /// after the resident reducer commits an event; standalone callers still
    /// advance them through the typed mutation methods below.
    pub(crate) fn set_counters(&mut self, sequence: u64, revision: u64) {
        self.sequence = sequence;
        self.revision = revision;
    }

    pub fn validate(&self) -> Result<(), StateError> {
        if self.epoch == 0 {
            return Err(StateError::invalid("epoch", "must be non-zero"));
        }

        for (scope_key, project) in &self.projects {
            project.validate()?;
            if scope_key != project.project_scope.as_str() {
                return Err(StateError::Invariant(format!(
                    "project key {scope_key} does not match scope {}",
                    project.project_scope.as_str()
                )));
            }
        }

        for (key, binding) in &self.current_thread_routes {
            binding.validate()?;
            let Some(session_id) = binding.session_id.as_ref() else {
                return Err(StateError::Invariant(format!(
                    "current thread route {:?} has no session id",
                    key
                )));
            };
            let Some(native_thread_id) = binding.native_thread_id.as_ref() else {
                return Err(StateError::Invariant(format!(
                    "current thread route {:?} has no native thread id",
                    key
                )));
            };
            if key
                != &current_thread_route_address(
                    session_id,
                    native_thread_id,
                    binding.tmux_endpoint.as_ref(),
                )
            {
                return Err(StateError::Invariant(format!(
                    "current thread route key {:?} does not match its session/thread/endpoint address",
                    key
                )));
            }
        }
        for ((app_key, thread_key, binding_key), binding) in &self.legacy_thread_routes {
            binding.validate()?;
            if binding.session_id.is_some() {
                return Err(StateError::Invariant(format!(
                    "legacy thread route {thread_key} must not carry a session id"
                )));
            }
            let Some(native_thread_id) = binding.native_thread_id.as_ref() else {
                return Err(StateError::Invariant(format!(
                    "legacy thread route {thread_key} has no native thread id"
                )));
            };
            if app_key != binding.app_scope_id.as_str()
                || thread_key != native_thread_id.as_str()
                || binding_key != binding.binding_id.as_str()
            {
                return Err(StateError::Invariant(format!(
                    "legacy thread route key {app_key}/{thread_key}/{binding_key} does not match its app scope, native thread, and binding"
                )));
            }
        }
        for (key, tombstone) in &self.current_thread_route_tombstones {
            tombstone.validate()?;
            if key
                != &current_route_address_key(
                    &tombstone.session_id,
                    &tombstone.native_thread_id,
                    tombstone.tmux_endpoint.as_ref(),
                )
            {
                return Err(StateError::Invariant(format!(
                    "current thread route tombstone key {key} does not match old route address {}/{}",
                    tombstone.session_id, tombstone.native_thread_id
                )));
            }
            let old_address = current_thread_route_address(
                &tombstone.session_id,
                &tombstone.native_thread_id,
                tombstone.tmux_endpoint.as_ref(),
            );
            if self.current_thread_routes.contains_key(&old_address) {
                return Err(StateError::Invariant(format!(
                    "current thread route tombstone {key} is also live"
                )));
            }
        }

        let mut operations = BTreeSet::new();
        for (command_key, receipt) in &self.command_receipts {
            receipt.validate()?;
            if command_key != receipt.command_id.as_str() {
                return Err(StateError::Invariant(format!(
                    "command receipt key {command_key} does not match command {}",
                    receipt.command_id
                )));
            }
            if receipt.epoch != self.epoch {
                return Err(StateError::Invariant(format!(
                    "command {command_key} belongs to epoch {}, expected {}",
                    receipt.epoch, self.epoch
                )));
            }
            if !operations.insert(receipt.operation_id.as_str().to_owned()) {
                return Err(StateError::Invariant(format!(
                    "operation {} is recorded more than once",
                    receipt.operation_id
                )));
            }
        }
        for (operation_key, evidence) in &self.migration_commit_evidence {
            evidence.validate()?;
            if operation_key != evidence.operation_id.as_str() {
                return Err(StateError::Invariant(format!(
                    "migration commit evidence key {operation_key} does not match operation {}",
                    evidence.operation_id
                )));
            }
            if evidence.target_epoch != self.epoch {
                return Err(StateError::Invariant(format!(
                    "migration commit evidence {} belongs to target epoch {}, expected {}",
                    evidence.operation_id, evidence.target_epoch, self.epoch
                )));
            }
            if evidence.committed_revision > self.revision {
                return Err(StateError::Invariant(format!(
                    "migration commit evidence {} revision {} exceeds global revision {}",
                    evidence.operation_id, evidence.committed_revision, self.revision
                )));
            }
            if !operations.insert(evidence.operation_id.as_str().to_owned()) {
                return Err(StateError::Invariant(format!(
                    "operation {} is recorded more than once",
                    evidence.operation_id
                )));
            }
        }
        Ok(())
    }

    /// Validate the target-side receipt graph against this projected global
    /// state.  This is intentionally read-only: epoch allocation and runtime
    /// rebind operations remain owned by the resident journal writer.
    pub fn validate_migration_receipts(
        &self,
        receipts: &MigrationReceiptSet,
    ) -> Result<(), StateError> {
        self.validate()?;
        let writer = receipts
            .writer
            .as_ref()
            .ok_or_else(|| StateError::invalid("migration writer receipt", "is required"))?;
        receipts.validate_for(
            &writer.migration_id,
            &writer.source_project_id,
            &writer.source_snapshot_digest,
            self.epoch,
        )?;

        let mut receipt_operation_ids = BTreeSet::new();
        self.validate_migration_commit_evidence(
            "writer",
            &writer.migration_id,
            &writer.source_project_id,
            &writer.source_snapshot_digest,
            &writer.archive_digest,
            writer.target_epoch,
            &writer.operation_id,
            None,
            writer.fencing_token,
            writer.committed_revision,
        )?;
        receipt_operation_ids.insert(writer.operation_id.as_str().to_owned());

        for receipt in &receipts.identity_rebinds {
            self.validate_migration_commit_evidence(
                "identity rebind",
                &receipt.migration_id,
                &receipt.source_project_id,
                &receipt.source_snapshot_digest,
                &writer.archive_digest,
                receipt.target_epoch,
                &receipt.operation_id,
                Some(&receipt.binding_id),
                receipt.fencing_token,
                receipt.committed_revision,
            )?;
            receipt_operation_ids.insert(receipt.operation_id.as_str().to_owned());
        }
        for receipt in &receipts.runtime_rebinds {
            self.validate_migration_commit_evidence(
                "runtime rebind",
                &receipt.migration_id,
                &receipt.source_project_id,
                &receipt.source_snapshot_digest,
                &writer.archive_digest,
                receipt.target_epoch,
                &receipt.operation_id,
                Some(&receipt.binding_id),
                receipt.fencing_token,
                receipt.committed_revision,
            )?;
            receipt_operation_ids.insert(receipt.operation_id.as_str().to_owned());
        }

        for evidence in self.migration_commit_evidence.values() {
            if evidence.migration_id == writer.migration_id
                && evidence.source_project_id == writer.source_project_id
                && evidence.source_snapshot_digest == writer.source_snapshot_digest
                && evidence.target_epoch == writer.target_epoch
                && !receipt_operation_ids.contains(evidence.operation_id.as_str())
            {
                return Err(StateError::Invariant(format!(
                    "migration commit evidence {} is not paired with a receipt",
                    evidence.operation_id
                )));
            }
        }

        let expected_bindings: Vec<&RuntimeBinding> = self
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .collect();
        if expected_bindings.len() != receipts.runtime_rebinds.len() {
            return Err(StateError::Invariant(format!(
                "runtime rebind receipt count {} does not match target binding count {}",
                receipts.runtime_rebinds.len(),
                expected_bindings.len()
            )));
        }

        for binding in expected_bindings {
            let Some(receipt) = receipts.runtime_rebinds.iter().find(|receipt| {
                receipt.project_scope == binding.project_scope
                    && receipt.app_scope_id == binding.app_scope_id
                    && receipt.agent_id == binding.agent_id
                    && receipt.runtime_id == binding.runtime_id
                    && receipt.binding_id == binding.binding_id
                    && receipt.endpoint_generation == binding.endpoint_generation
                    && receipt.native_thread_id == binding.native_thread_id
            }) else {
                return Err(StateError::Invariant(format!(
                    "target runtime binding {} has no matching migration receipt",
                    binding.binding_id
                )));
            };

            let route_scope = binding.route_scope();
            if self
                .lookup_binding_for(&route_scope, &receipt.binding_id)
                .is_none()
            {
                return Err(StateError::Invariant(format!(
                    "migration receipt {} is not bound to a registered route",
                    receipt.operation_id
                )));
            }
        }
        Ok(())
    }

    fn validate_migration_commit_evidence(
        &self,
        kind: &str,
        migration_id: &str,
        source_project_id: &str,
        source_snapshot_digest: &str,
        receipt_archive_digest: &str,
        target_epoch: u64,
        operation_id: &OperationId,
        binding_id: Option<&BindingId>,
        fencing_token: u64,
        committed_revision: u64,
    ) -> Result<(), StateError> {
        let Some(evidence) = self.migration_commit_evidence.get(operation_id.as_str()) else {
            return Err(StateError::Invariant(format!(
                "{kind} receipt {} has no authoritative migration commit evidence",
                operation_id
            )));
        };

        // The receipt owns the archive truth; evidence only projects it.  A
        // record-only edit therefore cannot satisfy this binding, and a
        // pre-fence receipt (empty digest, deserialized via `default`) is
        // refused explicitly instead of silently accepted.
        if receipt_archive_digest.trim().is_empty() {
            return Err(StateError::Invariant(format!(
                "{kind} receipt {} carries no authoritative archive digest",
                operation_id
            )));
        }
        if evidence.archive_digest != receipt_archive_digest {
            return Err(StateError::Invariant(format!(
                "{kind} receipt {} evidence archive digest {} does not match the receipt archive digest {}",
                operation_id, evidence.archive_digest, receipt_archive_digest
            )));
        }

        if evidence.migration_id != migration_id {
            return Err(StateError::Invariant(format!(
                "{kind} receipt {} evidence belongs to migration {}, expected {}",
                operation_id, evidence.migration_id, migration_id
            )));
        }
        if evidence.source_project_id != source_project_id {
            return Err(StateError::Invariant(format!(
                "{kind} receipt {} evidence belongs to source project {}, expected {}",
                operation_id, evidence.source_project_id, source_project_id
            )));
        }
        if evidence.source_snapshot_digest != source_snapshot_digest {
            return Err(StateError::Invariant(format!(
                "{kind} receipt {} evidence belongs to source digest {}, expected {}",
                operation_id, evidence.source_snapshot_digest, source_snapshot_digest
            )));
        }
        if evidence.target_epoch != target_epoch {
            return Err(StateError::Invariant(format!(
                "{kind} receipt {} evidence belongs to target epoch {}, expected {}",
                operation_id, evidence.target_epoch, target_epoch
            )));
        }
        if evidence.fencing_token != fencing_token {
            return Err(StateError::Invariant(format!(
                "{kind} receipt {} evidence uses fencing token {}, expected {}",
                operation_id, evidence.fencing_token, fencing_token
            )));
        }
        if evidence.committed_revision != committed_revision {
            return Err(StateError::Invariant(format!(
                "{kind} receipt {} evidence uses committed revision {}, expected {}",
                operation_id, evidence.committed_revision, committed_revision
            )));
        }
        match (evidence.binding_id.as_ref(), binding_id) {
            (None, None) => {}
            (Some(observed), Some(expected)) if observed == expected => {}
            (None, Some(expected)) => {
                return Err(StateError::Invariant(format!(
                    "{kind} receipt {} evidence has no binding, expected {}",
                    operation_id, expected
                )));
            }
            (Some(observed), None) => {
                return Err(StateError::Invariant(format!(
                    "{kind} receipt {} evidence is bound to {}, writer evidence must be unbound",
                    operation_id, observed
                )));
            }
            (Some(observed), Some(expected)) => {
                return Err(StateError::Invariant(format!(
                    "{kind} receipt {} evidence is bound to {}, expected {}",
                    operation_id, observed, expected
                )));
            }
        }
        Ok(())
    }

    /// Build the only canonical project key accepted by this model.  The
    /// caller supplies the registered project root; execution worktrees are
    /// intentionally not accepted as a substitute registration root.
    pub fn canonical_project_scope(path: &Path) -> Result<ProjectScopeId, StateError> {
        let canonical = std::fs::canonicalize(path).map_err(|error| {
            StateError::invalid(
                "project scope",
                format!("cannot canonicalize {}: {error}", path.display()),
            )
        })?;
        let text = canonical.to_str().ok_or_else(|| {
            StateError::invalid("project scope", "canonical path must be valid UTF-8")
        })?;
        ProjectScopeId::new(text.to_owned())
            .map_err(|error| StateError::invalid("project scope", error.to_string()))
    }

    pub fn lookup_project(&self, project_scope: &ProjectScopeId) -> Option<&ProjectState> {
        self.projects.get(project_scope.as_str())
    }

    /// Register one `(app_scope_id, project_scope_id)` route.  The route is
    /// the typed key used by future server routing; registration creation
    /// remains the only operation that creates a project entry.
    pub fn register_project_for_route(
        &mut self,
        route_scope: &RouteScope,
        registered_at_ms: i64,
    ) -> Result<StateVersion, StateError> {
        validate_route_scope(route_scope)?;
        let registration = ProjectRegistration::with_registered_at(
            route_scope.project_scope_id.clone(),
            route_scope.app_scope_id.clone(),
            registered_at_ms,
        )?;
        self.register_project(registration)
    }

    /// Look up a project only when the requested AppServer is registered for
    /// that project.  This prevents a known project from being treated as a
    /// valid route for an unknown AppServer scope.
    pub fn lookup_project_for_route(&self, route_scope: &RouteScope) -> Option<&ProjectState> {
        validate_route_scope(route_scope).ok()?;
        self.lookup_registration(&route_scope.project_scope_id, &route_scope.app_scope_id)
            .and_then(|_| self.lookup_project(&route_scope.project_scope_id))
    }

    pub fn lookup_registration(
        &self,
        project_scope: &ProjectScopeId,
        app_scope_id: &AppServerId,
    ) -> Option<&ProjectRegistration> {
        self.lookup_project(project_scope)
            .and_then(|project| project.lookup_registration(app_scope_id))
    }

    /// Look up an unscoped binding only when its ID is host-wide unique.
    /// Route-aware callers must use [`Self::lookup_binding_for`], because the
    /// same binding ID is allowed in two independent project scopes.
    pub fn lookup_binding(&self, binding_id: &BindingId) -> Option<&RuntimeBinding> {
        let mut found = None;
        for project in self.projects.values() {
            if let Some(binding) = project.lookup_binding(binding_id) {
                if found.is_some() {
                    // An ambiguous unscoped lookup must fail closed rather
                    // than selecting whichever project sorts first.
                    return None;
                }
                found = Some(binding);
            }
        }
        found
    }

    pub fn lookup_binding_for(
        &self,
        route_scope: &RouteScope,
        binding_id: &BindingId,
    ) -> Option<&RuntimeBinding> {
        self.lookup_project_for_route(route_scope)
            .and_then(|project| project.lookup_binding(binding_id))
            .filter(|binding| binding.app_scope_id == route_scope.app_scope_id)
    }

    pub fn lookup_current_thread_route(
        &self,
        session_id: &SessionId,
        native_thread_id: &NativeThreadId,
    ) -> Option<&RuntimeBinding> {
        let route_key = (
            session_id.as_str().to_owned(),
            native_thread_id.as_str().to_owned(),
        );
        if let Some(binding) = self.current_thread_routes.get(&route_key) {
            return Some(binding);
        }
        let mut matches = self.current_thread_routes.values().filter(|binding| {
            binding.session_id.as_ref() == Some(session_id)
                && binding.native_thread_id.as_ref() == Some(native_thread_id)
        });
        let binding = matches.next()?;
        matches.next().is_none().then_some(binding)
    }

    pub fn lookup_tmux_route(&self, endpoint: &TmuxEndpoint) -> Option<&RuntimeBinding> {
        if let Some(binding) = self
            .current_thread_routes
            .get(&tmux_route_address_for_lookup(endpoint))
        {
            return Some(binding);
        }
        // Pane-only recovery may carry no Codex session/thread IDs.  An App
        // Server route stores those native IDs on the binding while keeping
        // the full pane tuple as the last-resort recovery anchor, so a direct
        // native-key lookup misses it.  Only scan when the caller is pane-only
        // and there is exactly one matching pane, otherwise fail closed.
        if endpoint.codex_session_id.is_none() && endpoint.codex_thread_id.is_none() {
            let mut matches = self.current_thread_routes.values().filter(|binding| {
                binding.tmux_endpoint.as_ref().is_some_and(|persisted| {
                    crate::client::adapters::tmux::same_pane_route(persisted, endpoint)
                })
            });
            let binding = matches.next()?;
            return matches.next().is_none().then_some(binding);
        }
        None
    }

    pub fn lookup_tmux_route_tombstone(
        &self,
        endpoint: &TmuxEndpoint,
    ) -> Option<&RuntimeBindingTombstone> {
        self.current_thread_route_tombstones
            .get(&tmux_route_address_key_for_lookup(endpoint))
    }

    /// Read-only fallback candidates for a durable thread-only binding.
    ///
    /// A thread may appear under several binding ids when a disposable project
    /// reused the same peer thread.  The caller decides: exactly one candidate
    /// is recoverable, several are ambiguous and must fail closed, none means
    /// the thread has no legacy route.
    pub fn legacy_thread_route_matches(
        &self,
        native_thread_id: &NativeThreadId,
    ) -> Vec<&RuntimeBinding> {
        self.legacy_thread_routes
            .values()
            .filter(|binding| {
                binding
                    .native_thread_id
                    .as_ref()
                    .is_some_and(|thread| thread.as_str() == native_thread_id.as_str())
            })
            .collect()
    }

    /// Every session-bound binding that owns this native thread, across all
    /// projects.  A legacy candidate may only be used when the thread has no
    /// strict owner: a newer session-bound binding means the legacy record is
    /// superseded and a request carrying a different session must not be
    /// silently routed through the older project.
    pub fn strict_bindings_for_native_thread(
        &self,
        native_thread_id: &NativeThreadId,
    ) -> Vec<&RuntimeBinding> {
        self.projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .filter(|binding| {
                binding.session_id.is_some()
                    && binding.tmux_endpoint.is_none()
                    && binding
                        .native_thread_id
                        .as_ref()
                        .is_some_and(|thread| thread.as_str() == native_thread_id.as_str())
            })
            .collect()
    }

    /// Project one durable thread-only binding into the read-only
    /// compatibility index.  A higher `endpoint_generation` for the same app
    /// scope, thread, and binding is a transport refresh and replaces the
    /// stored record; a distinct binding id is kept alongside it.
    pub fn set_legacy_thread_route(
        &mut self,
        binding: RuntimeBinding,
    ) -> Result<StateVersion, StateError> {
        binding.validate()?;
        if binding.session_id.is_some() {
            return Err(StateError::invalid(
                "legacy thread route",
                "must not carry a session id",
            ));
        }
        let native_thread_id = binding.native_thread_id.clone().ok_or_else(|| {
            StateError::invalid("legacy thread route", "requires a native thread id")
        })?;
        let key = (
            binding.app_scope_id.as_str().to_owned(),
            native_thread_id.as_str().to_owned(),
            binding.binding_id.as_str().to_owned(),
        );
        if self
            .legacy_thread_routes
            .get(&key)
            .is_some_and(|existing| existing == &binding)
        {
            return Ok(self.version());
        }
        self.mutate(|next| {
            if let Some(existing) = next.legacy_thread_routes.get(&key) {
                if binding.endpoint_generation <= existing.endpoint_generation {
                    return Ok(());
                }
            }
            next.legacy_thread_routes.insert(key, binding);
            Ok(())
        })
    }

    pub fn lookup_current_thread_route_tombstone(
        &self,
        session_id: &SessionId,
        native_thread_id: &NativeThreadId,
    ) -> Option<&RuntimeBindingTombstone> {
        if let Some(tombstone) =
            self.current_thread_route_tombstones
                .get(&current_route_address_key(
                    session_id,
                    native_thread_id,
                    None,
                ))
        {
            return Some(tombstone);
        }
        let mut matches = self
            .current_thread_route_tombstones
            .values()
            .filter(|tombstone| {
                tombstone.session_id == *session_id
                    && tombstone.native_thread_id == *native_thread_id
            });
        let tombstone = matches.next()?;
        matches.next().is_none().then_some(tombstone)
    }

    pub fn record_current_thread_route_tombstone(
        &mut self,
        tombstone: RuntimeBindingTombstone,
    ) -> Result<StateVersion, StateError> {
        tombstone.validate()?;
        let key = current_route_address_key(
            &tombstone.session_id,
            &tombstone.native_thread_id,
            tombstone.tmux_endpoint.as_ref(),
        );
        if self
            .current_thread_route_tombstones
            .get(&key)
            .is_some_and(|existing| existing == &tombstone)
        {
            return Ok(self.version());
        }
        self.mutate(|next| {
            next.current_thread_route_tombstones.insert(key, tombstone);
            Ok(())
        })
    }

    /// Advance the one current route for one session/thread pair.
    ///
    /// Runtime history remains in `projects`; this index is the only route
    /// selector and retires the prior entry for the same binding.
    pub fn set_current_thread_route(
        &mut self,
        binding: RuntimeBinding,
    ) -> Result<StateVersion, StateError> {
        binding.validate()?;
        let native_thread_id = binding.native_thread_id.clone().ok_or_else(|| {
            StateError::invalid("current thread route", "requires a native thread id")
        })?;
        let session_id = binding
            .session_id
            .clone()
            .ok_or_else(|| StateError::invalid("current thread route", "requires a session id"))?;
        let route_address = current_thread_route_address(
            &session_id,
            &native_thread_id,
            binding.tmux_endpoint.as_ref(),
        );
        let app_scope_key = binding.app_scope_id.as_str().to_owned();
        let upgraded_binding_id = binding.binding_id.as_str().to_owned();
        let thread_key = binding
            .native_thread_id
            .as_ref()
            .map(|thread| thread.as_str().to_owned())
            .ok_or_else(|| {
                StateError::invalid("current thread route", "requires a native thread id")
            })?;
        if self.current_thread_routes.get(&route_address) == Some(&binding) {
            return Ok(self.version());
        }
        self.mutate(|next| {
            let retired = next
                .current_thread_routes
                .iter()
                .filter(|(_, existing)| {
                    existing.route_scope() == binding.route_scope()
                        && existing.binding_id == binding.binding_id
                        && existing != &&binding
                })
                .map(|(_, existing)| existing.clone())
                .collect::<Vec<_>>();
            next.current_thread_routes.retain(|_, existing| {
                existing.route_scope() != binding.route_scope()
                    || existing.binding_id != binding.binding_id
            });
            for old in retired {
                // A higher endpoint generation on the same session/thread is
                // a transport refresh, not an address rebind. Keep the route
                // current and do not create a tombstone for the same address.
                let old_session_id = old.session_id.clone().ok_or_else(|| {
                    StateError::invalid(
                        "current thread route tombstone",
                        "old binding has no session id",
                    )
                })?;
                let old_native_thread_id = old.native_thread_id.clone().ok_or_else(|| {
                    StateError::invalid(
                        "current thread route tombstone",
                        "old binding has no native thread id",
                    )
                })?;
                if current_thread_route_address(
                    &old_session_id,
                    &old_native_thread_id,
                    old.tmux_endpoint.as_ref(),
                ) == route_address
                {
                    continue;
                }
                let key = current_route_address_key(
                    &old_session_id,
                    &old_native_thread_id,
                    old.tmux_endpoint.as_ref(),
                );
                next.current_thread_route_tombstones
                    .insert(key, RuntimeBindingTombstone::new(&old, &binding)?);
            }
            next.current_thread_routes.insert(route_address, binding);
            // Installing the strict dual-key route for a thread upgrades that
            // identity off the legacy compatibility index.  Only the upgraded
            // binding is retired: another binding id may still own the same
            // thread as a distinct legacy candidate.
            let upgraded_binding_key = upgraded_binding_id.clone();
            next.legacy_thread_routes
                .retain(|(app, thread, binding_id), _| {
                    app != &app_scope_key
                        || thread != &thread_key
                        || binding_id != &upgraded_binding_key
                });
            Ok(())
        })
    }

    pub fn retire_current_thread_route(
        &mut self,
        binding: RuntimeBinding,
    ) -> Result<StateVersion, StateError> {
        binding.validate()?;
        let Some(native_thread_id) = binding.native_thread_id.as_ref() else {
            return Err(StateError::invalid(
                "current thread route retirement",
                "requires a native thread id",
            ));
        };
        let Some(session_id) = binding.session_id.as_ref() else {
            return Err(StateError::invalid(
                "current thread route retirement",
                "requires a session id",
            ));
        };
        let route_address = current_thread_route_address(
            session_id,
            native_thread_id,
            binding.tmux_endpoint.as_ref(),
        );
        self.mutate(|next| {
            if next.current_thread_routes.get(&route_address) == Some(&binding) {
                next.current_thread_routes.remove(&route_address);
            }
            Ok(())
        })
    }

    pub fn lookup_master_grant_for(
        &self,
        route_scope: &RouteScope,
        binding_id: &BindingId,
    ) -> Option<&MasterGrant> {
        self.lookup_project_for_route(route_scope)
            .and_then(|project| project.lookup_master_grant(binding_id))
            .filter(|grant| {
                grant.project_scope == route_scope.project_scope_id
                    && grant.app_scope_id == route_scope.app_scope_id
            })
    }

    pub fn lookup_master_grant(
        &self,
        project_scope: &ProjectScopeId,
        binding_id: &BindingId,
    ) -> Option<&MasterGrant> {
        self.lookup_project(project_scope)
            .and_then(|project| project.lookup_master_grant(binding_id))
    }

    pub fn lookup_command_receipt(&self, command_id: &CommandId) -> Option<&CommandReceipt> {
        self.command_receipts.get(command_id.as_str())
    }

    /// Install a receipt that was already committed by the host journal.
    ///
    /// The legacy reducer owns the journal sequence/revision while this
    /// host-wide map owns command idempotency.  Consequently the receipt's
    /// coordinates are validated as durable values but do not have to be
    /// bounded by this reducer's independent version counters.
    pub fn record_command_projection(&mut self, receipt: CommandReceipt) -> Result<(), StateError> {
        receipt.validate()?;
        if receipt.epoch != self.epoch {
            return Err(StateError::Invariant(format!(
                "command {} belongs to epoch {}, expected {}",
                receipt.command_id, receipt.epoch, self.epoch
            )));
        }
        if let Some(existing) = self.lookup_command_receipt(&receipt.command_id) {
            if existing == &receipt {
                return Ok(());
            }
            if existing.operation_id != receipt.operation_id {
                return Err(StateError::CommandIdReuse {
                    command_id: receipt.command_id.as_str().to_owned(),
                    existing_operation: existing.operation_id.as_str().to_owned(),
                    observed_operation: receipt.operation_id.as_str().to_owned(),
                });
            }
            return Err(StateError::ReceiptConflict(format!(
                "command {} was already projected with a different receipt",
                receipt.command_id
            )));
        }
        if let Some(existing) = self
            .command_receipts
            .values()
            .find(|current| current.operation_id == receipt.operation_id)
        {
            return Err(StateError::OperationIdReuse {
                operation_id: receipt.operation_id.as_str().to_owned(),
                existing_command: existing.command_id.as_str().to_owned(),
            });
        }
        self.command_receipts
            .insert(receipt.command_id.as_str().to_owned(), receipt);
        Ok(())
    }

    /// Install one migration commit evidence record already committed by the
    /// resident journal.  The outer reducer owns the event and version
    /// ordering, so this projection deliberately does not bump or validate
    /// the independent global counters until the caller synchronizes them.
    pub fn record_migration_commit_evidence(
        &mut self,
        evidence: MigrationCommitEvidence,
    ) -> Result<(), StateError> {
        evidence.validate()?;
        if evidence.target_epoch != self.epoch {
            return Err(StateError::Invariant(format!(
                "migration commit evidence {} belongs to target epoch {}, expected {}",
                evidence.operation_id, evidence.target_epoch, self.epoch
            )));
        }
        let operation_key = evidence.operation_id.as_str().to_owned();
        if let Some(existing) = self.migration_commit_evidence.get(&operation_key) {
            if existing == &evidence {
                return Ok(());
            }
            return Err(StateError::ReceiptConflict(format!(
                "migration operation {} was already projected with different commit evidence",
                evidence.operation_id
            )));
        }
        if let Some(existing) = self
            .command_receipts
            .values()
            .find(|current| current.operation_id == evidence.operation_id)
        {
            return Err(StateError::OperationIdReuse {
                operation_id: evidence.operation_id.as_str().to_owned(),
                existing_command: existing.command_id.as_str().to_owned(),
            });
        }
        self.migration_commit_evidence
            .insert(operation_key, evidence);
        Ok(())
    }

    /// A new registration advances the host version exactly once.  Repeating
    /// the same registration is idempotent; a conflicting registration is
    /// rejected so one AppServer cannot silently replace another identity.
    pub fn register_project(
        &mut self,
        registration: ProjectRegistration,
    ) -> Result<StateVersion, StateError> {
        registration.validate()?;
        let scope_key = registration.project_scope.as_str().to_owned();
        let app_key = registration.app_scope_id.as_str().to_owned();
        if self
            .lookup_registration(&registration.project_scope, &registration.app_scope_id)
            .is_some_and(|current| current == &registration)
        {
            return Ok(self.version());
        }
        if self
            .lookup_registration(&registration.project_scope, &registration.app_scope_id)
            .is_some()
        {
            return Err(StateError::RegistrationConflict {
                project_scope: scope_key,
                app_scope_id: app_key,
            });
        }

        self.mutate(|next| {
            let project = next
                .projects
                .entry(scope_key.clone())
                .or_insert_with(|| ProjectState {
                    project_scope: registration.project_scope.clone(),
                    registrations: BTreeMap::new(),
                    runtime_bindings: BTreeMap::new(),
                    master_grants: BTreeMap::new(),
                });
            project.registrations.insert(app_key.clone(), registration);
            Ok(())
        })
    }

    /// Register or reconnect a runtime binding.  An older generation is
    /// rejected before state mutation.  A newer generation may replace a
    /// binding with the same principal; reconnecting revokes any grant tied
    /// to the old generation and requires an explicit new grant.
    pub fn bind_runtime(&mut self, binding: RuntimeBinding) -> Result<StateVersion, StateError> {
        binding.validate()?;
        let scope_key = binding.project_scope.as_str().to_owned();
        let binding_key = binding.binding_id.as_str().to_owned();
        let app_key = binding.app_scope_id.as_str().to_owned();

        let project = self
            .lookup_project(&binding.project_scope)
            .ok_or_else(|| StateError::ProjectNotRegistered(scope_key.clone()))?;
        if project.lookup_registration(&binding.app_scope_id).is_none() {
            return Err(StateError::ProjectNotRegistered(format!(
                "{} (app scope {})",
                binding.project_scope.as_str(),
                binding.app_scope_id
            )));
        }

        if let Some(current) = project.lookup_binding(&binding.binding_id) {
            if current == &binding {
                return Ok(self.version());
            }
            if !current.same_principal(&binding) {
                return Err(StateError::BindingConflict(format!(
                    "binding {} changes project, app scope, or agent",
                    binding.binding_id
                )));
            }
            if binding.endpoint_generation < current.endpoint_generation {
                return Err(StateError::StaleBinding {
                    binding_id: binding_key,
                    expected_generation: current.endpoint_generation,
                    observed_generation: binding.endpoint_generation,
                });
            }
            if binding.endpoint_generation == current.endpoint_generation {
                return Err(StateError::BindingConflict(format!(
                    "binding {} has a different runtime at generation {}",
                    binding.binding_id, binding.endpoint_generation
                )));
            }
        }

        let duplicate_runtime = project.runtime_bindings.values().find(|current| {
            current.runtime_id == binding.runtime_id && current.binding_id != binding.binding_id
        });
        if let Some(current) = duplicate_runtime {
            return Err(StateError::BindingConflict(format!(
                "runtime {} is already bound as {}",
                current.runtime_id, current.binding_id
            )));
        }

        self.mutate(|next| {
            let project = next
                .projects
                .get_mut(&scope_key)
                .ok_or_else(|| StateError::ProjectNotRegistered(scope_key.clone()))?;
            // The registration check above is repeated on the candidate so
            // this method remains safe if its mutation body is reused.
            if !project.registrations.contains_key(&app_key) {
                return Err(StateError::ProjectNotRegistered(format!(
                    "{} (app scope {})",
                    binding.project_scope.as_str(),
                    binding.app_scope_id
                )));
            }
            project
                .runtime_bindings
                .insert(binding_key.clone(), binding);
            // A capability is fenced to the endpoint generation.  Rebinding
            // the same binding ID therefore revokes the old capability in
            // the same candidate transaction.
            project.master_grants.remove(&binding_key);
            Ok(())
        })
    }

    /// Restore one exact runtime binding after a registration transaction
    /// fails to publish its host route. This is not a general rollback: the
    /// failed binding must still be current and the previous binding must be
    /// the same principal with a lower generation.
    pub fn rollback_runtime_binding(
        &mut self,
        failed: RuntimeBinding,
        previous: Option<RuntimeBinding>,
        previous_grant: Option<MasterGrant>,
    ) -> Result<StateVersion, StateError> {
        failed.validate()?;
        if let Some(previous) = &previous {
            previous.validate()?;
            if !previous.same_principal(&failed) {
                return Err(StateError::BindingConflict(format!(
                    "rollback binding {} does not preserve the registered principal",
                    failed.binding_id
                )));
            }
            if previous.endpoint_generation >= failed.endpoint_generation {
                return Err(StateError::StaleBinding {
                    binding_id: failed.binding_id.as_str().to_owned(),
                    expected_generation: failed.endpoint_generation,
                    observed_generation: previous.endpoint_generation,
                });
            }
        }
        if let Some(grant) = &previous_grant {
            grant.validate()?;
            let Some(previous) = previous.as_ref() else {
                return Err(StateError::MasterGrantBindingMismatch(format!(
                    "grant {} has no previous runtime binding",
                    grant.binding_id
                )));
            };
            if grant.project_scope != previous.project_scope
                || grant.app_scope_id != previous.app_scope_id
                || grant.agent_id != previous.agent_id
                || grant.binding_id != previous.binding_id
                || grant.endpoint_generation != previous.endpoint_generation
            {
                return Err(StateError::MasterGrantBindingMismatch(format!(
                    "grant {} does not match the previous runtime binding",
                    grant.binding_id
                )));
            }
        }

        let scope_key = failed.project_scope.as_str().to_owned();
        let binding_key = failed.binding_id.as_str().to_owned();
        let project = self
            .lookup_project(&failed.project_scope)
            .ok_or_else(|| StateError::ProjectNotRegistered(scope_key.clone()))?;
        let current = project
            .lookup_binding(&failed.binding_id)
            .ok_or_else(|| StateError::BindingNotFound(binding_key.clone()))?;
        if current != &failed {
            return Err(StateError::BindingConflict(format!(
                "binding {} is not the failed generation being rolled back",
                failed.binding_id
            )));
        }
        let current_grant = project.lookup_master_grant(&failed.binding_id);
        if let Some(grant) = current_grant {
            // A same-principal recovery reissues the master grant for the new
            // generation, so the failed transaction legitimately owns a grant
            // at exactly this tuple.  Any other grant still fails closed.
            if grant.project_scope != failed.project_scope
                || grant.app_scope_id != failed.app_scope_id
                || grant.agent_id != failed.agent_id
                || grant.binding_id != failed.binding_id
                || grant.endpoint_generation != failed.endpoint_generation
            {
                return Err(StateError::MasterGrantConflict(format!(
                    "binding {} master grant is not the failed transaction state",
                    failed.binding_id
                )));
            }
        }

        self.mutate(|next| {
            let project = next
                .projects
                .get_mut(&scope_key)
                .ok_or_else(|| StateError::ProjectNotRegistered(scope_key.clone()))?;
            match previous {
                Some(previous) => {
                    project
                        .runtime_bindings
                        .insert(binding_key.clone(), previous);
                }
                None => {
                    project.runtime_bindings.remove(&binding_key);
                }
            }
            match previous_grant {
                Some(grant) => {
                    project.master_grants.insert(binding_key.clone(), grant);
                }
                None => {
                    project.master_grants.remove(&binding_key);
                }
            }
            Ok(())
        })
    }

    /// Validate an actor binding against the current host state.  This is a
    /// pure lookup and never repairs or advances a stale binding.
    pub fn validate_binding(&self, incoming: &RuntimeBinding) -> Result<(), StateError> {
        incoming.validate()?;
        let route_scope = incoming.route_scope();
        let Some(current) = self.lookup_binding_for(&route_scope, &incoming.binding_id) else {
            return Err(StateError::BindingNotFound(
                incoming.binding_id.as_str().to_owned(),
            ));
        };
        if !current.same_principal(incoming) {
            return Err(StateError::BindingConflict(format!(
                "binding {} does not belong to the registered principal",
                incoming.binding_id
            )));
        }
        if incoming.endpoint_generation < current.endpoint_generation {
            return Err(StateError::StaleBinding {
                binding_id: incoming.binding_id.as_str().to_owned(),
                expected_generation: current.endpoint_generation,
                observed_generation: incoming.endpoint_generation,
            });
        }
        if incoming.endpoint_generation != current.endpoint_generation {
            return Err(StateError::BindingConflict(format!(
                "binding {} generation {} is not current generation {}",
                incoming.binding_id, incoming.endpoint_generation, current.endpoint_generation
            )));
        }
        if incoming != current {
            return Err(StateError::BindingConflict(format!(
                "binding {} identity differs from the registered endpoint",
                incoming.binding_id
            )));
        }
        Ok(())
    }

    pub fn grant_master(&mut self, grant: MasterGrant) -> Result<StateVersion, StateError> {
        grant.validate()?;
        let scope_key = grant.project_scope.as_str().to_owned();
        let binding_key = grant.binding_id.as_str().to_owned();
        let project = self
            .lookup_project(&grant.project_scope)
            .ok_or_else(|| StateError::ProjectNotRegistered(scope_key.clone()))?;
        let Some(binding) = project.lookup_binding(&grant.binding_id) else {
            return Err(StateError::BindingNotFound(binding_key));
        };
        if binding.app_scope_id != grant.app_scope_id || binding.agent_id != grant.agent_id {
            return Err(StateError::MasterGrantBindingMismatch(format!(
                "grant {} targets a different app scope or agent",
                grant.binding_id
            )));
        }
        if grant.endpoint_generation != binding.endpoint_generation {
            return Err(StateError::StaleBinding {
                binding_id: grant.binding_id.as_str().to_owned(),
                expected_generation: binding.endpoint_generation,
                observed_generation: grant.endpoint_generation,
            });
        }
        if let Some(current) = project.lookup_master_grant(&grant.binding_id) {
            if current == &grant {
                return Ok(self.version());
            }
            return Err(StateError::MasterGrantConflict(format!(
                "binding {} already has a different grant",
                grant.binding_id
            )));
        }
        if let Some(current) = project.master_grants.values().find(|current| {
            current.project_scope == grant.project_scope
                && current.app_scope_id == grant.app_scope_id
        }) {
            return Err(StateError::MasterGrantConflict(format!(
                "route {} / {} already has a master grant for {}",
                current.project_scope.as_str(),
                current.app_scope_id.as_str(),
                current.agent_id.as_str()
            )));
        }

        self.mutate(|next| {
            let project = next
                .projects
                .get_mut(&scope_key)
                .ok_or_else(|| StateError::ProjectNotRegistered(scope_key.clone()))?;
            project.master_grants.insert(binding_key, grant);
            Ok(())
        })
    }

    /// Revoke the current master capability for one scoped binding.  The
    /// operation is idempotent, while a later reconnect also removes the
    /// grant automatically through `bind_runtime`.
    pub fn revoke_master(
        &mut self,
        project_scope: &ProjectScopeId,
        binding_id: &BindingId,
    ) -> Result<StateVersion, StateError> {
        validate_project_scope(project_scope)?;
        validate_binding_id(binding_id)?;
        let scope_key = project_scope.as_str().to_owned();
        let project = self
            .lookup_project(project_scope)
            .ok_or_else(|| StateError::ProjectNotRegistered(scope_key.clone()))?;
        if project.lookup_master_grant(binding_id).is_none() {
            return Ok(self.version());
        }

        self.mutate(|next| {
            let project = next
                .projects
                .get_mut(&scope_key)
                .ok_or_else(|| StateError::ProjectNotRegistered(scope_key.clone()))?;
            project.master_grants.remove(binding_id.as_str());
            Ok(())
        })
    }

    pub fn role_for_binding(
        &self,
        project_scope: &ProjectScopeId,
        binding_id: &BindingId,
    ) -> PeerRole {
        let Some(project) = self.lookup_project(project_scope) else {
            return PeerRole::Peer;
        };
        let Some(binding) = project.lookup_binding(binding_id) else {
            return PeerRole::Peer;
        };
        project
            .lookup_master_grant(binding_id)
            .filter(|grant| grant.endpoint_generation == binding.endpoint_generation)
            .map_or(PeerRole::Peer, |_| PeerRole::Master)
    }

    pub fn role_for_route(&self, route_scope: &RouteScope, binding_id: &BindingId) -> PeerRole {
        let Some(binding) = self.lookup_binding_for(route_scope, binding_id) else {
            return PeerRole::Peer;
        };
        self.lookup_master_grant_for(route_scope, binding_id)
            .filter(|grant| grant.endpoint_generation == binding.endpoint_generation)
            .map_or(PeerRole::Peer, |_| PeerRole::Master)
    }

    pub fn record_command(
        &mut self,
        command_id: CommandId,
        operation_id: OperationId,
        outcome: Value,
    ) -> Result<CommandReceipt, StateError> {
        validate_command_id(&command_id)?;
        validate_operation_id(&operation_id)?;
        if let Some(existing) = self.lookup_command_receipt(&command_id) {
            if existing.operation_id != operation_id {
                return Err(StateError::CommandIdReuse {
                    command_id: command_id.as_str().to_owned(),
                    existing_operation: existing.operation_id.as_str().to_owned(),
                    observed_operation: operation_id.as_str().to_owned(),
                });
            }
            if existing.outcome == outcome {
                return Ok(existing.clone());
            }
            return Err(StateError::ReceiptConflict(format!(
                "command {} was already recorded with a different outcome",
                command_id
            )));
        }
        if let Some(existing) = self
            .command_receipts
            .values()
            .find(|receipt| receipt.operation_id == operation_id)
        {
            return Err(StateError::OperationIdReuse {
                operation_id: operation_id.as_str().to_owned(),
                existing_command: existing.command_id.as_str().to_owned(),
            });
        }

        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(StateError::CounterOverflow("sequence"))?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(StateError::CounterOverflow("revision"))?;
        let receipt = CommandReceipt {
            command_id: command_id.clone(),
            operation_id,
            epoch: self.epoch,
            sequence,
            revision,
            outcome,
        };
        receipt.validate()?;
        let committed = receipt.clone();
        self.mutate_with(|next| {
            next.command_receipts
                .insert(command_id.as_str().to_owned(), receipt);
            Ok(committed)
        })
        .map(|(receipt, _)| receipt)
    }

    /// Compare-and-swap is the caller-visible transaction fence.  The
    /// mutation runs on a clone and is published only after invariants pass;
    /// a failed closure, validation error or stale expected revision leaves
    /// this value untouched.  The closure edits project data, while the
    /// helper owns the one host-wide sequence/revision increment.
    pub fn compare_and_swap<F>(
        &mut self,
        expected_revision: u64,
        mutate: F,
    ) -> Result<StateVersion, StateError>
    where
        F: FnOnce(&mut GlobalState) -> Result<(), StateError>,
    {
        if self.revision != expected_revision {
            return Err(StateError::CompareAndSwapMismatch {
                expected: expected_revision,
                observed: self.revision,
            });
        }
        let mut next = self.clone();
        mutate(&mut next)?;
        // Counter and epoch ownership stays with this helper.  Direct writes
        // to those fields inside the closure are ignored rather than allowed
        // to forge a host ordering value.
        next.epoch = self.epoch;
        next.sequence = self.sequence;
        next.revision = self.revision;
        let version = next.bump_counters()?;
        next.validate()?;
        *self = next;
        Ok(version)
    }

    fn mutate<F>(&mut self, mutate: F) -> Result<StateVersion, StateError>
    where
        F: FnOnce(&mut GlobalState) -> Result<(), StateError>,
    {
        self.mutate_with(|next| {
            mutate(next)?;
            Ok(())
        })
        .map(|(_, version)| version)
    }

    fn mutate_with<R, F>(&mut self, mutate: F) -> Result<(R, StateVersion), StateError>
    where
        F: FnOnce(&mut GlobalState) -> Result<R, StateError>,
    {
        let mut next = self.clone();
        let result = mutate(&mut next)?;
        let version = next.bump_counters()?;
        next.validate()?;
        *self = next;
        Ok((result, version))
    }

    fn bump_counters(&mut self) -> Result<StateVersion, StateError> {
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(StateError::CounterOverflow("sequence"))?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(StateError::CounterOverflow("revision"))?;
        self.sequence = sequence;
        self.revision = revision;
        Ok(self.version())
    }
}

fn validate_project_scope(scope: &ProjectScopeId) -> Result<(), StateError> {
    ProjectScopeId::new(scope.as_str().to_owned())
        .map_err(|error| StateError::invalid("project scope", error.to_string()))
        .map(|_| ())
}

fn current_thread_route_address(
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

fn tmux_route_address(endpoint: &TmuxEndpoint) -> (String, String) {
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

fn tmux_route_address_for_lookup(endpoint: &TmuxEndpoint) -> (String, String) {
    match (&endpoint.codex_session_id, &endpoint.codex_thread_id) {
        (Some(session), Some(thread)) => (session.clone(), thread.clone()),
        _ => tmux_route_address(endpoint),
    }
}

fn current_route_address_key(
    session_id: &SessionId,
    native_thread_id: &NativeThreadId,
    tmux_endpoint: Option<&TmuxEndpoint>,
) -> String {
    let address = current_thread_route_address(session_id, native_thread_id, tmux_endpoint);
    format!("{}\0{}", address.0, address.1)
}

fn tmux_route_address_key(endpoint: &TmuxEndpoint) -> String {
    let address = tmux_route_address(endpoint);
    format!("{}\0{}", address.0, address.1)
}

fn tmux_route_address_key_for_lookup(endpoint: &TmuxEndpoint) -> String {
    let address = tmux_route_address_for_lookup(endpoint);
    format!("{}\0{}", address.0, address.1)
}

fn validate_tmux_route_endpoint(endpoint: &TmuxEndpoint) -> Result<(), StateError> {
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

fn validate_route_scope(scope: &RouteScope) -> Result<(), StateError> {
    validate_app_scope(&scope.app_scope_id)?;
    validate_project_scope(&scope.project_scope_id)
}

fn validate_app_scope(scope: &AppServerId) -> Result<(), StateError> {
    AppServerId::new(scope.as_str().to_owned())
        .map_err(|error| StateError::invalid("app scope", error.to_string()))
        .map(|_| ())
}

fn validate_agent_id(id: &AgentId) -> Result<(), StateError> {
    AgentId::new(id.as_str().to_owned())
        .map_err(|error| StateError::invalid("agent id", error.to_string()))
        .map(|_| ())
}

fn validate_runtime_id(id: &RuntimeId) -> Result<(), StateError> {
    RuntimeId::new(id.as_str().to_owned())
        .map_err(|error| StateError::invalid("runtime id", error.to_string()))
        .map(|_| ())
}

fn validate_binding_id(id: &BindingId) -> Result<(), StateError> {
    BindingId::new(id.as_str().to_owned())
        .map_err(|error| StateError::invalid("binding id", error.to_string()))
        .map(|_| ())
}

fn validate_native_thread_id(id: &NativeThreadId) -> Result<(), StateError> {
    NativeThreadId::new(id.as_str().to_owned())
        .map_err(|error| StateError::invalid("native thread id", error.to_string()))
        .map(|_| ())
}

fn validate_session_id(id: &SessionId) -> Result<(), StateError> {
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

fn validate_command_id(id: &CommandId) -> Result<(), StateError> {
    CommandId::new(id.as_str().to_owned())
        .map_err(|error| StateError::invalid("command id", error.to_string()))
        .map(|_| ())
}

fn validate_operation_id(id: &OperationId) -> Result<(), StateError> {
    OperationId::new(id.as_str().to_owned())
        .map_err(|error| StateError::invalid("operation id", error.to_string()))
        .map(|_| ())
}

fn validate_non_empty_text(field: &'static str, value: &str) -> Result<(), StateError> {
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

fn validate_migration_identifier(field: &'static str, value: &str) -> Result<(), StateError> {
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

fn validate_migration_epoch(
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
fn validate_migration_receipt_identity(
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
fn validate_migration_context(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn project_scope() -> ProjectScopeId {
        GlobalState::canonical_project_scope(Path::new(env!("CARGO_MANIFEST_DIR")))
            .expect("repository root is canonical")
    }

    fn app_scope(id: &str) -> AppServerId {
        AppServerId::new(id).expect("app scope id")
    }

    fn registration(scope: &ProjectScopeId, app: &str) -> ProjectRegistration {
        ProjectRegistration::new(scope.clone(), app_scope(app)).expect("registration")
    }

    fn binding(
        scope: &ProjectScopeId,
        app: &str,
        agent: &str,
        runtime: &str,
        binding_id: &str,
        generation: u64,
    ) -> RuntimeBinding {
        RuntimeBinding::new(
            scope.clone(),
            app_scope(app),
            AgentId::new(agent).unwrap(),
            RuntimeId::new(runtime).unwrap(),
            BindingId::new(binding_id).unwrap(),
            generation,
            None,
        )
        .expect("binding")
    }

    fn grant(
        scope: &ProjectScopeId,
        app: &str,
        agent: &str,
        binding_id: &str,
        generation: u64,
    ) -> MasterGrant {
        MasterGrant::new(
            scope.clone(),
            app_scope(app),
            AgentId::new(agent).unwrap(),
            "task-scoped",
            "operator",
            "user approved",
            BindingId::new(binding_id).unwrap(),
            generation,
            1,
        )
        .expect("master grant")
    }

    #[test]
    fn different_projects_are_stored_without_overwriting_each_other() {
        let root = project_scope();
        let second = ProjectScopeId::new(format!("{}/second", root.as_str())).unwrap();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&root, "app-one"))
            .unwrap();
        state
            .register_project(registration(&second, "app-two"))
            .unwrap();

        assert_eq!(state.projects.len(), 2);
        assert!(state.lookup_project(&root).is_some());
        assert!(state.lookup_project(&second).is_some());
        state.validate().unwrap();
    }

    #[test]
    fn route_registration_is_idempotent_and_conflicts_keep_the_original() {
        let scope = project_scope();
        let route = RouteScope {
            app_scope_id: app_scope("app-one"),
            project_scope_id: scope.clone(),
        };
        let mut state = GlobalState::default();
        let first = state
            .register_project_for_route(&route, 11)
            .expect("first route registration");
        let replay = state
            .register_project_for_route(&route, 11)
            .expect("identical route registration is idempotent");
        assert_eq!(replay, first);
        assert_eq!(state.projects.len(), 1);

        let conflict = state.register_project_for_route(&route, 12);
        assert!(matches!(
            conflict,
            Err(StateError::RegistrationConflict {
                project_scope,
                app_scope_id
            }) if project_scope == scope.as_str() && app_scope_id == "app-one"
        ));
        assert_eq!(
            state
                .lookup_registration(&scope, &route.app_scope_id)
                .unwrap()
                .registered_at_ms,
            11
        );
        state.validate().unwrap();
    }

    #[test]
    fn route_lookup_rejects_unknown_project_or_app_scope() {
        let scope = project_scope();
        let unknown_project =
            ProjectScopeId::new(format!("{}/unknown", scope.as_str())).expect("scope");
        let known_route = RouteScope {
            app_scope_id: app_scope("app-one"),
            project_scope_id: scope.clone(),
        };
        let unknown_app_route = RouteScope {
            app_scope_id: app_scope("app-unknown"),
            project_scope_id: scope.clone(),
        };
        let unknown_project_route = RouteScope {
            app_scope_id: app_scope("app-one"),
            project_scope_id: unknown_project.clone(),
        };
        let mut state = GlobalState::default();
        state
            .register_project_for_route(&known_route, 1)
            .expect("known route registration");

        assert!(state.lookup_project_for_route(&unknown_app_route).is_none());
        assert!(state
            .lookup_project_for_route(&unknown_project_route)
            .is_none());
        let binding = binding(
            &unknown_project,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            1,
        );
        let before = state.version();
        assert!(matches!(
            state.bind_runtime(binding),
            Err(StateError::ProjectNotRegistered(_))
        ));
        assert_eq!(state.version(), before);
        state.validate().unwrap();
    }

    #[test]
    fn same_project_different_app_scopes_keep_separate_registrations_and_bindings() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        state
            .register_project(registration(&scope, "app-two"))
            .unwrap();
        state
            .bind_runtime(binding(
                &scope,
                "app-one",
                "agent-one",
                "runtime-one",
                "binding-one",
                1,
            ))
            .unwrap();
        state
            .bind_runtime(binding(
                &scope,
                "app-two",
                "agent-two",
                "runtime-two",
                "binding-two",
                1,
            ))
            .unwrap();

        let project = state.lookup_project(&scope).unwrap();
        assert_eq!(project.registrations.len(), 2);
        assert_eq!(project.runtime_bindings.len(), 2);
        assert_eq!(
            project.registrations["app-one"].app_scope_id.as_str(),
            "app-one"
        );
        assert_eq!(
            project.registrations["app-two"].app_scope_id.as_str(),
            "app-two"
        );
        assert_eq!(
            state
                .lookup_binding_for(
                    &binding(
                        &scope,
                        "app-two",
                        "agent-two",
                        "runtime-two",
                        "binding-two",
                        1
                    )
                    .route_scope(),
                    &BindingId::new("binding-two").unwrap()
                )
                .unwrap()
                .app_scope_id
                .as_str(),
            "app-two"
        );
        state.validate().unwrap();
    }

    #[test]
    fn same_binding_id_in_different_projects_is_route_scoped() {
        let first_scope = project_scope();
        let second_scope = ProjectScopeId::new(format!("{}/second", first_scope.as_str()))
            .expect("second project scope");
        let mut state = GlobalState::default();
        state
            .register_project(registration(&first_scope, "app-one"))
            .unwrap();
        state
            .register_project(registration(&second_scope, "app-one"))
            .unwrap();

        let first = binding(
            &first_scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "shared-binding",
            1,
        );
        let second = binding(
            &second_scope,
            "app-one",
            "agent-two",
            "runtime-two",
            "shared-binding",
            1,
        );
        state.bind_runtime(first.clone()).unwrap();
        state.bind_runtime(second.clone()).unwrap();

        let shared_id = BindingId::new("shared-binding").unwrap();
        assert!(state.lookup_binding(&shared_id).is_none());
        assert_eq!(
            state.lookup_binding_for(&first.route_scope(), &shared_id),
            Some(&first)
        );
        assert_eq!(
            state.lookup_binding_for(&second.route_scope(), &shared_id),
            Some(&second)
        );
        state.validate_binding(&first).unwrap();
        state.validate_binding(&second).unwrap();
        state.validate().unwrap();
    }

    #[test]
    fn master_grants_are_isolated_by_project_route_and_generation() {
        let first_scope = project_scope();
        let second_scope = ProjectScopeId::new(format!("{}/second", first_scope.as_str()))
            .expect("second project scope");
        let first_route = RouteScope {
            app_scope_id: app_scope("app-one"),
            project_scope_id: first_scope.clone(),
        };
        let second_route = RouteScope {
            app_scope_id: app_scope("app-one"),
            project_scope_id: second_scope.clone(),
        };
        let mut state = GlobalState::default();
        state.register_project_for_route(&first_route, 1).unwrap();
        state.register_project_for_route(&second_route, 2).unwrap();
        let first_binding = binding(
            &first_scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "shared-binding",
            3,
        );
        let second_binding = binding(
            &second_scope,
            "app-one",
            "agent-two",
            "runtime-two",
            "shared-binding",
            4,
        );
        state.bind_runtime(first_binding.clone()).unwrap();
        state.bind_runtime(second_binding.clone()).unwrap();
        state
            .grant_master(grant(
                &first_scope,
                "app-one",
                "agent-one",
                "shared-binding",
                3,
            ))
            .unwrap();
        state
            .grant_master(grant(
                &second_scope,
                "app-one",
                "agent-two",
                "shared-binding",
                4,
            ))
            .unwrap();

        let shared_id = BindingId::new("shared-binding").unwrap();
        assert_eq!(
            state
                .lookup_master_grant_for(&first_route, &shared_id)
                .unwrap()
                .project_scope,
            first_scope
        );
        assert_eq!(
            state
                .lookup_master_grant_for(&second_route, &shared_id)
                .unwrap()
                .project_scope,
            second_scope
        );
        assert_eq!(
            state.role_for_route(&first_route, &shared_id),
            PeerRole::Master
        );
        assert_eq!(
            state.role_for_route(&second_route, &shared_id),
            PeerRole::Master
        );
        let wrong_route = RouteScope {
            app_scope_id: app_scope("app-unknown"),
            project_scope_id: first_scope,
        };
        assert_eq!(
            state.role_for_route(&wrong_route, &shared_id),
            PeerRole::Peer
        );
        state.validate().unwrap();
    }

    #[test]
    fn sequence_and_revision_advance_together_and_cas_is_fenced() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        assert_eq!(
            state.version(),
            StateVersion {
                epoch: 1,
                sequence: 0,
                revision: 0
            }
        );
        let first = state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let second = state
            .bind_runtime(binding(
                &scope,
                "app-one",
                "agent-one",
                "runtime-one",
                "binding-one",
                1,
            ))
            .unwrap();
        assert_eq!((first.sequence, first.revision), (1, 1));
        assert_eq!((second.sequence, second.revision), (2, 2));

        let third = state
            .compare_and_swap(second.revision, |next| {
                next.projects
                    .get_mut(scope.as_str())
                    .unwrap()
                    .registrations
                    .get_mut("app-one")
                    .unwrap()
                    .registered_at_ms = 7;
                Ok(())
            })
            .unwrap();
        assert_eq!((third.sequence, third.revision), (3, 3));
        assert!(matches!(
            state.compare_and_swap(2, |_| Ok(())),
            Err(StateError::CompareAndSwapMismatch {
                expected: 2,
                observed: 3
            })
        ));
        state.validate().unwrap();
    }

    #[test]
    fn old_generation_is_rejected_without_mutating_state() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let current = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            2,
        );
        state.bind_runtime(current.clone()).unwrap();
        let before = state.version();
        let stale = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            1,
        );
        assert!(matches!(
            state.validate_binding(&stale),
            Err(StateError::StaleBinding {
                expected_generation: 2,
                observed_generation: 1,
                ..
            })
        ));
        assert!(matches!(
            state.bind_runtime(stale),
            Err(StateError::StaleBinding {
                expected_generation: 2,
                observed_generation: 1,
                ..
            })
        ));
        assert_eq!(state.version(), before);
        assert_eq!(
            state
                .lookup_binding(&current.binding_id)
                .unwrap()
                .endpoint_generation,
            2
        );
    }

    #[test]
    fn registration_defaults_to_peer_until_an_explicit_current_grant() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let runtime = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            4,
        );
        state.bind_runtime(runtime.clone()).unwrap();
        assert_eq!(
            state.role_for_binding(&scope, &runtime.binding_id),
            PeerRole::Peer
        );

        state
            .grant_master(grant(&scope, "app-one", "agent-one", "binding-one", 4))
            .unwrap();
        assert_eq!(
            state.role_for_binding(&scope, &runtime.binding_id),
            PeerRole::Master
        );
        state.validate().unwrap();
    }

    #[test]
    fn route_rejects_a_second_master_grant() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let first = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            1,
        );
        let second = binding(
            &scope,
            "app-one",
            "agent-two",
            "runtime-two",
            "binding-two",
            1,
        );
        state.bind_runtime(first.clone()).unwrap();
        state.bind_runtime(second.clone()).unwrap();
        state
            .grant_master(grant(&scope, "app-one", "agent-one", "binding-one", 1))
            .unwrap();

        assert!(matches!(
            state.grant_master(grant(&scope, "app-one", "agent-two", "binding-two", 1,)),
            Err(StateError::MasterGrantConflict(_))
        ));
        assert_eq!(
            state.role_for_binding(&scope, &first.binding_id),
            PeerRole::Master
        );
        assert_eq!(
            state.role_for_binding(&scope, &second.binding_id),
            PeerRole::Peer
        );
        state.validate().unwrap();
    }

    #[test]
    fn reconnect_revokes_old_master_grant_and_explicit_revoke_is_idempotent() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let current = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            4,
        );
        state.bind_runtime(current.clone()).unwrap();
        state
            .grant_master(grant(&scope, "app-one", "agent-one", "binding-one", 4))
            .unwrap();
        assert_eq!(
            state.role_for_binding(&scope, &current.binding_id),
            PeerRole::Master
        );

        let reconnected = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            5,
        );
        state.bind_runtime(reconnected.clone()).unwrap();
        assert!(state
            .lookup_master_grant(&scope, &reconnected.binding_id)
            .is_none());
        assert_eq!(
            state.role_for_binding(&scope, &reconnected.binding_id),
            PeerRole::Peer
        );
        assert!(matches!(
            state.grant_master(grant(&scope, "app-one", "agent-one", "binding-one", 4)),
            Err(StateError::StaleBinding {
                expected_generation: 5,
                observed_generation: 4,
                ..
            })
        ));
        state
            .grant_master(grant(&scope, "app-one", "agent-one", "binding-one", 5))
            .unwrap();
        assert_eq!(
            state.role_for_binding(&scope, &reconnected.binding_id),
            PeerRole::Master
        );

        let before_revoke = state.version();
        state
            .revoke_master(&scope, &reconnected.binding_id)
            .unwrap();
        assert_eq!(
            state.role_for_binding(&scope, &reconnected.binding_id),
            PeerRole::Peer
        );
        assert!(state
            .lookup_master_grant(&scope, &reconnected.binding_id)
            .is_none());
        let after_revoke = state.version();
        assert!(after_revoke.revision > before_revoke.revision);
        state
            .revoke_master(&scope, &reconnected.binding_id)
            .unwrap();
        assert_eq!(state.version(), after_revoke);
        state.validate_binding(&reconnected).unwrap();
        state.validate().unwrap();
    }

    #[test]
    fn runtime_binding_rollback_restores_only_the_exact_previous_generation_and_grant() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let previous = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            4,
        );
        let previous_grant = grant(&scope, "app-one", "agent-one", "binding-one", 4);
        state.bind_runtime(previous.clone()).unwrap();
        state.grant_master(previous_grant.clone()).unwrap();
        let failed = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-two",
            "binding-one",
            5,
        );
        state.bind_runtime(failed.clone()).unwrap();
        // A same-principal recovery reissues the grant for the new
        // generation, which is exactly the transaction rollback must undo.
        let failed_grant = grant(&scope, "app-one", "agent-one", "binding-one", 5);
        state.grant_master(failed_grant.clone()).unwrap();
        assert_eq!(
            state.lookup_master_grant(&scope, &failed.binding_id),
            Some(&failed_grant)
        );

        state
            .rollback_runtime_binding(
                failed.clone(),
                Some(previous.clone()),
                Some(previous_grant.clone()),
            )
            .unwrap();
        assert_eq!(state.lookup_binding(&previous.binding_id), Some(&previous));
        assert_eq!(
            state.lookup_master_grant(&scope, &previous.binding_id),
            Some(&previous_grant)
        );
        assert_eq!(
            state.role_for_binding(&scope, &previous.binding_id),
            PeerRole::Master
        );
        state.validate().unwrap();

        let before = state.version();
        assert!(matches!(
            state.rollback_runtime_binding(
                failed,
                Some(previous.clone()),
                Some(previous_grant.clone()),
            ),
            Err(StateError::BindingConflict(_))
        ));
        assert_eq!(state.version(), before);
    }

    #[test]
    fn runtime_binding_rollback_rejects_a_grant_that_is_not_the_failed_transaction_state() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let previous = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-one",
            "binding-one",
            4,
        );
        state.bind_runtime(previous.clone()).unwrap();
        let failed = binding(
            &scope,
            "app-one",
            "agent-one",
            "runtime-two",
            "binding-one",
            5,
        );
        state.bind_runtime(failed.clone()).unwrap();

        // A grant on the same binding whose generation does not match the
        // failed transaction is not this transaction's state, so rollback
        // must still fail closed instead of silently restoring it.
        let mismatched = MasterGrant::new(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            "project",
            "operator",
            "user approved",
            BindingId::new("binding-one").unwrap(),
            failed.endpoint_generation + 1,
            1,
        )
        .unwrap();
        let mut with_mismatched = state.clone();
        with_mismatched
            .projects
            .get_mut(scope.as_str())
            .unwrap()
            .master_grants
            .insert(failed.binding_id.as_str().to_owned(), mismatched);
        assert!(matches!(
            with_mismatched.rollback_runtime_binding(failed.clone(), Some(previous), None),
            Err(StateError::MasterGrantConflict(_))
        ));
        assert_eq!(with_mismatched.version(), state.version());
        state.validate().unwrap();
    }

    #[test]
    fn legacy_thread_route_is_indexed_read_only_and_upgraded_by_a_strict_route() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-legacy-upgrade").unwrap();
        let legacy = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-legacy").unwrap(),
            RuntimeId::new("runtime-legacy").unwrap(),
            BindingId::new("binding-legacy").unwrap(),
            1,
            None,
            Some(thread_id.clone()),
        )
        .unwrap();

        state.bind_runtime(legacy.clone()).unwrap();
        state.set_legacy_thread_route(legacy.clone()).unwrap();
        assert_eq!(state.legacy_thread_route_matches(&thread_id), vec![&legacy]);
        assert!(state
            .lookup_current_thread_route(
                &SessionId::new("session-legacy-upgrade").unwrap(),
                &thread_id
            )
            .is_none());
        state.validate().unwrap();

        // A strict dual-key route for the same thread is the upgrade; the
        // legacy record must not remain as a second live selector.
        let strict = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-legacy").unwrap(),
            RuntimeId::new("runtime-legacy").unwrap(),
            BindingId::new("binding-legacy").unwrap(),
            2,
            Some(SessionId::new("session-legacy-upgrade").unwrap()),
            Some(thread_id.clone()),
        )
        .unwrap();
        state.bind_runtime(strict.clone()).unwrap();
        state.set_current_thread_route(strict.clone()).unwrap();
        assert!(state.legacy_thread_route_matches(&thread_id).is_empty());
        assert_eq!(
            state.lookup_current_thread_route(
                &SessionId::new("session-legacy-upgrade").unwrap(),
                &thread_id
            ),
            Some(&strict)
        );
        state.validate().unwrap();
    }

    #[test]
    fn legacy_thread_route_keeps_distinct_bindings_for_one_thread_as_candidates() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-legacy-candidates").unwrap();
        let first = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            None,
            Some(thread_id.clone()),
        )
        .unwrap();
        let second = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-two").unwrap(),
            RuntimeId::new("runtime-two").unwrap(),
            BindingId::new("binding-two").unwrap(),
            1,
            None,
            Some(thread_id.clone()),
        )
        .unwrap();
        state.set_legacy_thread_route(first.clone()).unwrap();
        state.set_legacy_thread_route(second.clone()).unwrap();

        // Both candidates are retained; the resolver must fail closed rather
        // than pick one.
        let mut matches = state.legacy_thread_route_matches(&thread_id);
        matches.sort_by(|left, right| left.binding_id.as_str().cmp(right.binding_id.as_str()));
        assert_eq!(matches, vec![&first, &second]);
        state.validate().unwrap();
    }

    #[test]
    fn upgrading_one_legacy_binding_keeps_the_other_candidate_resolvable() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-legacy-sibling").unwrap();
        let legacy = |agent: &str, binding_id: &str, runtime_id: &str| {
            RuntimeBinding::new_with_session(
                scope.clone(),
                app_scope("app-one"),
                AgentId::new(agent).unwrap(),
                RuntimeId::new(runtime_id).unwrap(),
                BindingId::new(binding_id).unwrap(),
                1,
                None,
                Some(thread_id.clone()),
            )
            .unwrap()
        };
        let first = legacy("agent-one", "binding-one", "runtime-one");
        let second = legacy("agent-two", "binding-two", "runtime-two");
        state.bind_runtime(first.clone()).unwrap();
        state.bind_runtime(second.clone()).unwrap();
        state.set_legacy_thread_route(first.clone()).unwrap();
        state.set_legacy_thread_route(second.clone()).unwrap();

        // Upgrade only the first identity to a strict dual-key route.  The
        // sibling binding id must stay resolvable rather than be swept away.
        let strict = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            2,
            Some(SessionId::new("session-legacy-sibling").unwrap()),
            Some(thread_id.clone()),
        )
        .unwrap();
        state.bind_runtime(strict.clone()).unwrap();
        state.set_current_thread_route(strict.clone()).unwrap();

        assert_eq!(state.legacy_thread_route_matches(&thread_id), vec![&second]);
        assert_eq!(
            state.lookup_current_thread_route(
                &SessionId::new("session-legacy-sibling").unwrap(),
                &thread_id
            ),
            Some(&strict)
        );
        state.validate().unwrap();
    }

    #[test]
    fn a_session_bound_thread_is_reported_as_strictly_owned() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-strict-owner").unwrap();

        // A legacy record for the thread and a distinct strict binding for the
        // same thread must both be visible, so the resolver can refuse to use
        // the legacy fallback once the thread has a live strict owner.
        let legacy = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-legacy").unwrap(),
            RuntimeId::new("runtime-legacy").unwrap(),
            BindingId::new("binding-legacy").unwrap(),
            1,
            None,
            Some(thread_id.clone()),
        )
        .unwrap();
        let strict = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-strict").unwrap(),
            RuntimeId::new("runtime-strict").unwrap(),
            BindingId::new("binding-strict").unwrap(),
            1,
            Some(SessionId::new("session-strict-owner").unwrap()),
            Some(thread_id.clone()),
        )
        .unwrap();
        state.bind_runtime(legacy.clone()).unwrap();
        state.bind_runtime(strict.clone()).unwrap();
        state.set_legacy_thread_route(legacy.clone()).unwrap();

        assert_eq!(
            state.strict_bindings_for_native_thread(&thread_id),
            vec![&strict]
        );
        assert_eq!(state.legacy_thread_route_matches(&thread_id), vec![&legacy]);
        state.validate().unwrap();
    }

    #[test]
    fn legacy_thread_route_refresh_replaces_the_same_binding() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-legacy-refresh").unwrap();
        let binding = |generation: u64| {
            RuntimeBinding::new_with_session(
                scope.clone(),
                app_scope("app-one"),
                AgentId::new("agent-one").unwrap(),
                RuntimeId::new("runtime-one").unwrap(),
                BindingId::new("binding-one").unwrap(),
                generation,
                None,
                Some(thread_id.clone()),
            )
            .unwrap()
        };
        state.set_legacy_thread_route(binding(1)).unwrap();
        state.set_legacy_thread_route(binding(5)).unwrap();
        // A lower or equal generation must not roll the record back.
        state.set_legacy_thread_route(binding(3)).unwrap();

        let matches = state.legacy_thread_route_matches(&thread_id);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].endpoint_generation, 5);
        state.validate().unwrap();
    }

    #[test]
    fn pane_only_lookup_finds_appserver_binding_by_pane_recovery_anchor() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-pane-only-appserver").unwrap();
        let mut strict = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-pane-appserver").unwrap(),
            RuntimeId::new("runtime-pane-appserver").unwrap(),
            BindingId::new("binding-pane-appserver").unwrap(),
            1,
            Some(SessionId::new("session-pane-appserver").unwrap()),
            Some(thread_id.clone()),
        )
        .unwrap();
        strict.tmux_endpoint = Some(crate::proto::TmuxEndpoint {
            socket_path: "/tmp/collab-pane-appserver.sock".into(),
            server_pid: 11,
            tmux_session_id: "$9".into(),
            pane_id: "%33".into(),
            pane_pid: 55,
            codex_session_id: Some("session-pane-appserver".into()),
            codex_thread_id: Some("thread-pane-only-appserver".into()),
        });
        state.bind_runtime(strict.clone()).unwrap();
        state.set_current_thread_route(strict.clone()).unwrap();

        let pane_only = crate::proto::TmuxEndpoint {
            socket_path: "/tmp/collab-pane-appserver.sock".into(),
            server_pid: 11,
            tmux_session_id: "$9".into(),
            pane_id: "%33".into(),
            pane_pid: 55,
            codex_session_id: None,
            codex_thread_id: None,
        };
        assert_eq!(
            state.lookup_tmux_route(&pane_only),
            Some(&strict),
            "pane-only lookup must find the App Server route's persisted pane recovery anchor"
        );
        state.validate().unwrap();
    }

    #[test]
    fn current_thread_routes_keep_distinct_sessions_for_one_native_thread() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-shared").unwrap();
        let first = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            Some(SessionId::new("session-one").unwrap()),
            Some(thread_id.clone()),
        )
        .unwrap();
        let second = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-two").unwrap(),
            RuntimeId::new("runtime-two").unwrap(),
            BindingId::new("binding-two").unwrap(),
            1,
            Some(SessionId::new("session-two").unwrap()),
            Some(thread_id.clone()),
        )
        .unwrap();

        state.bind_runtime(first.clone()).unwrap();
        state.bind_runtime(second.clone()).unwrap();
        state.set_current_thread_route(first.clone()).unwrap();
        state.set_current_thread_route(second.clone()).unwrap();

        assert_eq!(
            state.lookup_current_thread_route(first.session_id.as_ref().unwrap(), &thread_id,),
            Some(&first)
        );
        assert_eq!(
            state.lookup_current_thread_route(second.session_id.as_ref().unwrap(), &thread_id,),
            Some(&second)
        );
        state.validate().unwrap();
    }

    #[test]
    fn same_address_generation_refresh_replaces_the_route_without_a_tombstone() {
        let scope = project_scope();
        let mut state = GlobalState::default();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        let thread_id = NativeThreadId::new("thread-refresh").unwrap();
        let session_id = SessionId::new("session-refresh").unwrap();
        let first = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            Some(session_id.clone()),
            Some(thread_id.clone()),
        )
        .unwrap();
        let refreshed = RuntimeBinding::new_with_session(
            scope.clone(),
            app_scope("app-one"),
            AgentId::new("agent-one").unwrap(),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            2,
            Some(session_id.clone()),
            Some(thread_id.clone()),
        )
        .unwrap();

        state.bind_runtime(first.clone()).unwrap();
        state.set_current_thread_route(first).unwrap();
        state.bind_runtime(refreshed.clone()).unwrap();
        state.set_current_thread_route(refreshed.clone()).unwrap();

        assert_eq!(
            state.lookup_current_thread_route(&session_id, &thread_id),
            Some(&refreshed)
        );
        assert!(state
            .lookup_current_thread_route_tombstone(&session_id, &thread_id)
            .is_none());
        state.validate().unwrap();
    }

    #[test]
    fn command_receipts_are_host_wide_and_idempotent() {
        let mut state = GlobalState::default();
        let first = state
            .record_command(
                CommandId::new("command-one").unwrap(),
                OperationId::new("operation-one").unwrap(),
                serde_json::json!({"ok": true}),
            )
            .unwrap();
        let before = state.version();
        let replay = state
            .record_command(
                CommandId::new("command-one").unwrap(),
                OperationId::new("operation-one").unwrap(),
                serde_json::json!({"ok": true}),
            )
            .unwrap();
        assert_eq!(first, replay);
        assert_eq!(state.version(), before);
        assert!(matches!(
            state.record_command(
                CommandId::new("command-one").unwrap(),
                OperationId::new("operation-two").unwrap(),
                Value::Null,
            ),
            Err(StateError::CommandIdReuse { .. })
        ));
        state.validate().unwrap();
    }

    fn migration_writer(target_epoch: u64) -> MigrationWriterReceipt {
        MigrationWriterReceipt::new(
            "migration-1",
            "project-1",
            None,
            target_epoch,
            "sha256:source",
            AgentId::new("writer-1").unwrap(),
            OperationId::new("writer-op-1").unwrap(),
            7,
            1,
            1,
        )
        .map(|receipt| receipt.with_archive_digest("sha256:archive"))
        .expect("migration writer receipt")
    }

    fn migration_commit(
        operation_id: &str,
        binding_id: Option<&str>,
        committed_revision: u64,
    ) -> MigrationCommitEvidence {
        MigrationCommitEvidence::new(
            "migration-1",
            "project-1",
            2,
            "sha256:source",
            OperationId::new(operation_id).expect("operation id"),
            binding_id.map(|value| BindingId::new(value).expect("binding id")),
            7,
            committed_revision,
        )
        .map(|evidence| evidence.with_archive_digest("sha256:archive"))
        .expect("migration commit evidence")
    }

    #[test]
    fn migration_receipt_set_requires_one_writer_and_unique_rebind_pairs() {
        let missing = MigrationReceiptSet::default();
        assert!(matches!(
            missing.validate(),
            Err(StateError::Invalid {
                field: "migration writer receipt",
                ..
            })
        ));

        let mut writer = migration_writer(2);
        writer.writer_count = 2;
        assert!(writer.validate().is_err());

        let mut receipts = MigrationReceiptSet {
            writer: Some(migration_writer(2)),
            ..MigrationReceiptSet::default()
        };
        let scope = project_scope();
        let identity = MigrationIdentityRebindReceipt::new(
            "migration-1",
            "project-1",
            scope.clone(),
            None,
            2,
            "sha256:source",
            AgentId::new("agent-1").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            OperationId::new("identity-op-1").unwrap(),
            7,
            2,
        )
        .unwrap();
        let runtime = MigrationRuntimeRebindReceipt::new(
            "migration-1",
            "project-1",
            scope,
            None,
            2,
            "sha256:source",
            AgentId::new("agent-other").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            None,
            OperationId::new("runtime-op-1").unwrap(),
            7,
            3,
        )
        .unwrap();
        receipts.identity_rebinds.push(identity);
        receipts.runtime_rebinds.push(runtime);
        assert!(matches!(
            receipts.validate(),
            Err(StateError::Invariant(reason)) if reason.contains("matching runtime receipt")
        ));
    }

    #[test]
    fn global_state_checks_migration_runtime_receipts_against_target_bindings() {
        let scope = project_scope();
        let mut state = GlobalState::new(2).unwrap();
        state
            .register_project(registration(&scope, "app-one"))
            .unwrap();
        state
            .bind_runtime(binding(
                &scope,
                "app-one",
                "agent-one",
                "runtime-one",
                "binding-one",
                1,
            ))
            .unwrap();

        let mut receipts = MigrationReceiptSet {
            writer: Some(migration_writer(2)),
            ..MigrationReceiptSet::default()
        };
        receipts.identity_rebinds.push(
            MigrationIdentityRebindReceipt::new(
                "migration-1",
                "project-1",
                scope.clone(),
                None,
                2,
                "sha256:source",
                AgentId::new("agent-one").unwrap(),
                app_scope("app-one"),
                RuntimeId::new("runtime-one").unwrap(),
                BindingId::new("binding-one").unwrap(),
                1,
                OperationId::new("identity-op-1").unwrap(),
                7,
                2,
            )
            .unwrap(),
        );
        receipts.runtime_rebinds.push(
            MigrationRuntimeRebindReceipt::new(
                "migration-1",
                "project-1",
                scope,
                None,
                2,
                "sha256:source",
                AgentId::new("agent-one").unwrap(),
                app_scope("app-one"),
                RuntimeId::new("runtime-one").unwrap(),
                BindingId::new("binding-one").unwrap(),
                1,
                None,
                OperationId::new("runtime-op-1").unwrap(),
                7,
                3,
            )
            .unwrap(),
        );
        state.set_counters(3, 3);
        state.migration_commit_evidence.insert(
            "writer-op-1".into(),
            migration_commit("writer-op-1", None, 1),
        );
        state.migration_commit_evidence.insert(
            "identity-op-1".into(),
            migration_commit("identity-op-1", Some("binding-one"), 2),
        );
        state.migration_commit_evidence.insert(
            "runtime-op-1".into(),
            migration_commit("runtime-op-1", Some("binding-one"), 3),
        );
        state.validate_migration_receipts(&receipts).unwrap();

        let runtime_evidence = state
            .migration_commit_evidence
            .remove("runtime-op-1")
            .expect("runtime commit evidence");
        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason))
                if reason.contains("has no authoritative migration commit evidence")
        ));
        state
            .migration_commit_evidence
            .insert("runtime-op-1".into(), runtime_evidence);

        state
            .migration_commit_evidence
            .get_mut("runtime-op-1")
            .expect("runtime commit evidence")
            .committed_revision = 4;
        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason)) if reason.contains("exceeds global revision")
        ));
        state
            .migration_commit_evidence
            .get_mut("runtime-op-1")
            .expect("runtime commit evidence")
            .committed_revision = 3;

        state
            .migration_commit_evidence
            .get_mut("runtime-op-1")
            .expect("runtime commit evidence")
            .fencing_token = 8;
        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason)) if reason.contains("evidence uses fencing token")
        ));
        state
            .migration_commit_evidence
            .get_mut("runtime-op-1")
            .expect("runtime commit evidence")
            .fencing_token = 7;

        receipts.runtime_rebinds[0].native_thread_id =
            Some(NativeThreadId::new("native-thread-one").expect("native thread id"));
        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason)) if reason.contains("no matching migration receipt")
        ));
        receipts.runtime_rebinds[0].native_thread_id = None;

        receipts.identity_rebinds[0].endpoint_generation = 2;
        receipts.runtime_rebinds[0].endpoint_generation = 2;
        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason)) if reason.contains("no matching migration receipt")
        ));
    }

    #[test]
    fn migration_receipts_require_one_source_epoch_and_a_symmetric_pairing() {
        assert!(MigrationWriterReceipt::new(
            "migration-1",
            "project-1",
            Some(0),
            2,
            "sha256:source",
            AgentId::new("writer-1").unwrap(),
            OperationId::new("writer-op-1").unwrap(),
            7,
            1,
            1,
        )
        .is_err());

        let scope = project_scope();
        let identity = MigrationIdentityRebindReceipt::new(
            "migration-1",
            "project-1",
            scope.clone(),
            None,
            2,
            "sha256:source",
            AgentId::new("agent-one").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            OperationId::new("identity-op-1").unwrap(),
            7,
            2,
        )
        .unwrap();
        let duplicate_identity = MigrationIdentityRebindReceipt::new(
            "migration-1",
            "project-1",
            scope.clone(),
            None,
            2,
            "sha256:source",
            AgentId::new("agent-one").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            OperationId::new("identity-op-2").unwrap(),
            7,
            3,
        )
        .unwrap();
        let runtime = MigrationRuntimeRebindReceipt::new(
            "migration-1",
            "project-1",
            scope.clone(),
            None,
            2,
            "sha256:source",
            AgentId::new("agent-one").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            None,
            OperationId::new("runtime-op-1").unwrap(),
            7,
            4,
        )
        .unwrap();
        let unmatched_runtime = MigrationRuntimeRebindReceipt::new(
            "migration-1",
            "project-1",
            scope,
            None,
            2,
            "sha256:source",
            AgentId::new("agent-two").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-two").unwrap(),
            BindingId::new("binding-two").unwrap(),
            1,
            None,
            OperationId::new("runtime-op-2").unwrap(),
            7,
            5,
        )
        .unwrap();
        let receipts = MigrationReceiptSet {
            writer: Some(migration_writer(2)),
            identity_rebinds: vec![identity, duplicate_identity],
            runtime_rebinds: vec![runtime, unmatched_runtime],
        };

        assert!(matches!(
            receipts.validate(),
            Err(StateError::Invariant(reason))
                if reason.contains("no unique matching identity receipt")
        ));
    }

    #[test]
    fn migration_receipts_reject_a_rebind_with_a_different_source_epoch() {
        let scope = project_scope();
        let identity = MigrationIdentityRebindReceipt::new(
            "migration-1",
            "project-1",
            scope.clone(),
            Some(1),
            2,
            "sha256:source",
            AgentId::new("agent-one").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            OperationId::new("identity-op-1").unwrap(),
            7,
            2,
        )
        .unwrap();
        let runtime = MigrationRuntimeRebindReceipt::new(
            "migration-1",
            "project-1",
            scope,
            Some(1),
            2,
            "sha256:source",
            AgentId::new("agent-one").unwrap(),
            app_scope("app-one"),
            RuntimeId::new("runtime-one").unwrap(),
            BindingId::new("binding-one").unwrap(),
            1,
            None,
            OperationId::new("runtime-op-1").unwrap(),
            7,
            3,
        )
        .unwrap();
        let receipts = MigrationReceiptSet {
            writer: Some(migration_writer(2)),
            identity_rebinds: vec![identity],
            runtime_rebinds: vec![runtime],
        };

        assert!(matches!(
            receipts.validate(),
            Err(StateError::Invariant(reason)) if reason.contains("different source epoch")
        ));
    }

    /// The receipt, not the record, owns the archive digest.  A record-only
    /// edit (the exact attack the reviewer reproduced) must be refused.
    #[test]
    fn migration_commit_fence_rejects_a_record_only_archive_digest_edit() {
        let mut state = GlobalState::new(2).unwrap();
        state.set_counters(9, 9);
        state.migration_commit_evidence.insert(
            "writer-op-1".into(),
            migration_commit("writer-op-1", None, 1)
                .with_archive_digest("sha256:attacker-supplied"),
        );
        let receipts = MigrationReceiptSet {
            writer: Some(migration_writer(2)),
            ..MigrationReceiptSet::default()
        };

        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason))
                if reason.contains("evidence archive digest sha256:attacker-supplied does not match the receipt archive digest sha256:archive")
        ));
    }

    /// A pre-fence record deserialized through `#[serde(default)]` has an empty
    /// archive digest and must be refused by the gate, not accepted.
    #[test]
    fn migration_commit_fence_rejects_an_unfenced_legacy_record_at_the_gate() {
        let mut state = GlobalState::new(2).unwrap();
        state.set_counters(9, 9);
        state.migration_commit_evidence.insert(
            "writer-op-1".into(),
            MigrationCommitEvidence::new(
                "migration-1",
                "project-1",
                2,
                "sha256:source",
                OperationId::new("writer-op-1").expect("operation id"),
                None,
                7,
                1,
            )
            .expect("legacy unfenced evidence deserializes"),
        );
        let receipts = MigrationReceiptSet {
            writer: Some(migration_writer(2)),
            ..MigrationReceiptSet::default()
        };

        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason))
                if reason.contains("evidence archive digest  does not match the receipt archive digest sha256:archive")
        ));
    }

    /// A receipt that carries no authoritative digest must be refused before
    /// any evidence comparison, so the fence fails closed on both sides.
    #[test]
    fn migration_commit_fence_rejects_a_receipt_without_an_archive_digest() {
        let mut state = GlobalState::new(2).unwrap();
        state.set_counters(9, 9);
        state.migration_commit_evidence.insert(
            "writer-op-1".into(),
            migration_commit("writer-op-1", None, 1),
        );
        let receipts = MigrationReceiptSet {
            writer: Some(
                MigrationWriterReceipt::new(
                    "migration-1",
                    "project-1",
                    None,
                    2,
                    "sha256:source",
                    AgentId::new("writer-1").unwrap(),
                    OperationId::new("writer-op-1").unwrap(),
                    7,
                    1,
                    1,
                )
                .expect("legacy receipt deserializes"),
            ),
            ..MigrationReceiptSet::default()
        };

        assert!(matches!(
            state.validate_migration_receipts(&receipts),
            Err(StateError::Invariant(reason))
                if reason.contains("carries no authoritative archive digest")
        ));
    }
}
