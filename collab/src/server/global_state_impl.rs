use super::global_state_helpers::*;
use super::global_state_models::*;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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
            ledger_scan_receipts: BTreeMap::new(),
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
        for receipt in self.ledger_scan_receipts.values() {
            receipt.validate()?;
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
            let mut matches = self.tmux_pane_route_claimants(endpoint).into_iter();
            let binding = matches.next()?;
            return matches.next().is_none().then_some(binding);
        }
        None
    }

    /// Every live route that claims this pane, host-wide.
    ///
    /// Pane uniqueness is a per-project contract, so this host-wide view is
    /// only for callers that genuinely ask a host-wide question.  Scope-aware
    /// callers use [`Self::tmux_pane_route_claimants_in_scope`].
    pub fn tmux_pane_route_claimants(&self, endpoint: &TmuxEndpoint) -> Vec<&RuntimeBinding> {
        self.current_thread_routes
            .values()
            .filter(|binding| {
                binding.tmux_endpoint.as_ref().is_some_and(|persisted| {
                    crate::client::adapters::tmux::same_pane_route(persisted, endpoint)
                })
            })
            .collect()
    }

    /// Every live route in `scope` that claims this pane.
    ///
    /// A claimant from another project scope is not a conflict: it neither
    /// blocks this scope nor may be retired by it.
    pub fn tmux_pane_route_claimants_in_scope(
        &self,
        scope: &RouteScope,
        endpoint: &TmuxEndpoint,
    ) -> Vec<&RuntimeBinding> {
        self.tmux_pane_route_claimants(endpoint)
            .into_iter()
            .filter(|binding| binding.route_scope() == *scope)
            .collect()
    }

    /// Recovery-only lookup of the complete pane address. The caller still
    /// authenticates the returned binding and checks its selected transport.
    pub fn lookup_unique_tmux_pane_route(
        &self,
        endpoint: &TmuxEndpoint,
    ) -> Option<&RuntimeBinding> {
        let mut matches = self.tmux_pane_route_claimants(endpoint).into_iter();
        let binding = matches.next()?;
        matches.next().is_none().then_some(binding)
    }

    /// Recovery-only lookup of the complete pane address inside one project
    /// scope.  Returns a route only when this scope has exactly one claimant.
    ///
    /// This is the scope-aware form used by the recovery fence, the reconciler,
    /// register recovery and the CLI rebind.  Two claimants inside one scope
    /// stay ambiguous and fail closed.
    pub fn lookup_unique_tmux_pane_route_in_scope(
        &self,
        scope: &RouteScope,
        endpoint: &TmuxEndpoint,
    ) -> Option<&RuntimeBinding> {
        let mut matches = self
            .tmux_pane_route_claimants_in_scope(scope, endpoint)
            .into_iter();
        let binding = matches.next()?;
        matches.next().is_none().then_some(binding)
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
}
