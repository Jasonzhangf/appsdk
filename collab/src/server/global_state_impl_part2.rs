use super::global_state_helpers::*;
use super::global_state_models::*;
use serde_json::Value;
use std::collections::BTreeMap;

impl GlobalState {
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
    /// selector. It retires the prior entry for the same binding, and it evicts
    /// every other claimant of the same tmux pane host-wide, so one pane owns
    /// exactly one binding.
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
            // An address that was tombstoned when it retired can legitimately
            // become live again under a later binding generation. That live
            // route and its stale tombstone must not coexist, so clear the
            // tombstone for the exact address we are re-activating.
            next.current_thread_route_tombstones.remove(&current_route_address_key(
                &session_id,
                &native_thread_id,
                binding.tmux_endpoint.as_ref(),
            ));
            // One tmux pane owns exactly one binding, host-wide. Installing this
            // route evicts every other claimant of the same pane, whatever its
            // project or route scope, so the later writer takes the resource.
            // This reducer is the same path for a live write and for journal
            // replay, so the invariant holds in both and needs no second
            // mechanism. An entry at this exact address is left for the insert
            // below; an entry without a pane is left alone, because it shares no
            // resource.
            if let Some(endpoint) = binding.tmux_endpoint.as_ref() {
                next.current_thread_routes.retain(|other_address, existing| {
                    other_address == &route_address
                        || existing.tmux_endpoint.as_ref().is_none_or(|other| {
                            !crate::client::adapters::tmux::same_owned_pane(other, endpoint)
                        })
                });
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

    pub fn record_ledger_scan_receipt(&mut self, receipt: LedgerScanReceipt) -> Result<StateVersion, StateError> {
        receipt.validate()?;
        // A scan receipt only inserts one entry into a map that no GlobalState
        // invariant inspects.  The clone-then-commit mutate path would copy the
        // whole unbounded receipt map (plus every project) for each event, so
        // replaying a journal with many receipts became quadratic.  Bump the
        // counters before the insert so a counter overflow still leaves the
        // state untouched.
        let version = self.bump_counters()?;
        self.ledger_scan_receipts.insert(receipt.scan_id.clone(), receipt);
        Ok(version)
    }

pub fn classify_runtime_binding_ledger(&mut self, record: RuntimeBindingLedgerRecord) -> Result<StateVersion, StateError> {
    if let Some(error) = self.runtime_binding_ledger_rejection(&record) {
        return Err(error);
    }
    self.mutate(|next| {
        if let Some(error) = next.runtime_binding_ledger_rejection(&record) {
            return Err(error);
        }
        let project = next.projects.get_mut(record.project_scope.as_str()).ok_or_else(|| StateError::ProjectNotRegistered(record.project_scope.as_str().to_owned()))?;
        project.runtime_binding_ledger.insert(record.key(), record);
        Ok(())
    })
}

/// Why [`Self::classify_runtime_binding_ledger`] rejects this record, or `None`
/// when it accepts it.
///
/// The ledger is resident control state, so a record is reducible here only
/// when it validates, its binding id names exactly one binding host-wide, the
/// project it names holds a registration for its app scope, and that project
/// holds a binding with its id whose principal coordinates agree. This is the
/// single source for that decision: the reducer's pre-check, the reducer's own
/// commit step, and any caller that must not append an unreducible record all
/// ask this one question, so no second copy of the rule can disagree with it.
pub fn runtime_binding_ledger_rejection(
    &self,
    record: &RuntimeBindingLedgerRecord,
) -> Option<StateError> {
    if let Err(error) = record.validate() {
        return Some(error);
    }
    if self
        .lookup_registration(&record.project_scope, &record.app_scope_id)
        .is_none()
    {
        return Some(StateError::ProjectNotRegistered(format!(
            "{} (app scope {})",
            record.project_scope.as_str(),
            record.app_scope_id
        )));
    }
    if self.lookup_binding(&record.binding_id).is_none() {
        return Some(StateError::BindingNotFound(
            record.binding_id.as_str().to_owned(),
        ));
    }
    let Some(project) = self.lookup_project(&record.project_scope) else {
        return Some(StateError::ProjectNotRegistered(
            record.project_scope.as_str().to_owned(),
        ));
    };
    let Some(binding) = project.lookup_binding(&record.binding_id) else {
        return Some(StateError::BindingNotFound(
            record.binding_id.as_str().to_owned(),
        ));
    };
    if binding.project_scope != record.project_scope
        || binding.app_scope_id != record.app_scope_id
        || binding.agent_id != record.agent_id
        || binding.runtime_id != record.runtime_id
    {
        return Some(StateError::Invariant(format!(
            "runtime binding ledger principal coordinates disagree for {}",
            record.binding_id
        )));
    }
    None
}

    pub fn lookup_runtime_binding_ledger(&self, project_scope: &ProjectScopeId, app_scope_id: &AppServerId, binding_id: &BindingId) -> Option<&RuntimeBindingLedgerRecord> {
        self.lookup_project(project_scope)
            .and_then(|project| project.lookup_registration(app_scope_id).map(|_| project))
            .and_then(|project| project.runtime_binding_ledger.get(binding_id.as_str()))
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
                    runtime_binding_ledger: BTreeMap::new(),
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
