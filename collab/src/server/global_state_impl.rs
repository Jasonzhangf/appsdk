use super::global_state_helpers::*;
use super::global_state_models::*;
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
        })
    }

    pub fn version(&self) -> StateVersion {
        StateVersion::from_state(self)
    }

    /// The current business owner controls the host version when this projection is
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
            if binding_route_address(binding).as_ref() != Some(key) {
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
    /// One pane owns exactly one binding host-wide, so this returns at most one
    /// route. The host-wide view is the only view: a claimant from another
    /// project is a conflict, not a neighbour. Ownership follows the pane
    /// address (`same_owned_pane`), so a reissued shell pid in the same pane is
    /// still the same owned pane.
    pub fn tmux_pane_route_claimants(&self, endpoint: &TmuxEndpoint) -> Vec<&RuntimeBinding> {
        self.current_thread_routes
            .values()
            .filter(|binding| {
                binding.tmux_endpoint.as_ref().is_some_and(|persisted| {
                    crate::client::adapters::tmux::same_owned_pane(persisted, endpoint)
                })
            })
            .collect()
    }

    /// The live route that claims this binding's pane as a *different* claim.
    ///
    /// One pane owns exactly one binding host-wide. Two claimants on the same
    /// pane are therefore never two owners: one of them is this binding's own
    /// route, and the other is the claim that took the pane.
    ///
    /// A generation refresh of the same principal and durable binding is this
    /// route, even when its stable pane address is unchanged. A claimant that
    /// is this same durable binding at another
    /// address is this route's own stale entry, and the repair is to advance it
    /// rather than to hand the pane to another project. Everything else on the
    /// pane is the claim that owns it.
    pub fn pane_claimant_other_than(&self, binding: &RuntimeBinding) -> Option<&RuntimeBinding> {
        let endpoint = binding.tmux_endpoint.as_ref()?;
        self.tmux_pane_route_claimants(endpoint)
            .into_iter()
            .find(|other| {
                let same_claim =
                    other.binding_id == binding.binding_id && other.same_principal(binding);
                !same_claim
            })
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
