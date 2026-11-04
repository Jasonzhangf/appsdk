impl ProjectRuntimeManager {
    fn binding_control_store(&self) -> crate::server::binding_store::BindingControlStore {
        let store = crate::server::binding_store::BindingControlStore::at(&self.route_journal);
        #[cfg(test)]
        if test_hooks::take_fail_next_parent_sync() {
            store.fail_parent_sync_on_next_publish();
        }
        store
    }

    pub(crate) fn commit_control_change(
        &self,
        change: crate::server::binding_store::ControlChange,
    ) -> Result<crate::server::binding_store::StoreCommit, String> {
        let _register = self.register_gate.lock().unwrap();
        self.commit_control_change_locked(change)
    }

    /// Commit one control change while the caller already holds the register
    /// gate. The name-decision critical section takes the gate once and then
    /// runs plan, control publish and business commit under a single
    /// project-state lock; this lock-free entry keeps that sequence from
    /// recursively acquiring the non-reentrant gate.
    pub(crate) fn commit_control_change_locked(
        &self,
        change: crate::server::binding_store::ControlChange,
    ) -> Result<crate::server::binding_store::StoreCommit, String> {
        self.binding_control_store().commit(change)
    }

    /// The exact current binding and selected transport for one worker, read
    /// from a caller-held state snapshot. Both the locking public entries and
    /// the locked control-first path share this so the key and the value come
    /// from the same state.
    fn control_binding_and_transport(
        state: &State,
        context: &ProjectContext,
        worker_id: &str,
        label: &str,
    ) -> Result<(RuntimeBinding, SelectedTransport), String> {
        let route_scope = RouteScope {
            app_scope_id: context.app_scope_id.clone(),
            project_scope_id: context.project_scope.clone(),
        };
        let binding = state
            .global
            .lookup_project_for_route(&route_scope)
            .and_then(|project| {
                project
                    .runtime_bindings
                    .values()
                    .filter(|binding| binding.agent_id.as_str() == worker_id)
                    .max_by_key(|binding| binding.endpoint_generation)
            })
            .cloned()
            .ok_or_else(|| {
                format!(
                    "BINDING_CONTROL_BINDING_MISSING: {label} for {worker_id} has no runtime binding"
                )
            })?;
        let worker = state.workers.get(worker_id).ok_or_else(|| {
            format!("BINDING_CONTROL_WORKER_MISSING: {label} for {worker_id} has no worker")
        })?;
        let transport = selected_transport_for_worker(worker).ok_or_else(|| {
            format!(
                "BINDING_CONTROL_TRANSPORT_MISSING: {label} for {worker_id} has no selected transport"
            )
        })?;
        Ok((binding, transport))
    }

    fn control_key_for_worker(
        &self,
        runtime: &Server,
        context: &ProjectContext,
        worker_id: &str,
        binding: &RuntimeBinding,
    ) -> Result<crate::server::binding_slot::IdentityBindingKey, String> {
        let state = runtime.state.lock().unwrap();
        let names = state
            .name_occupancies
            .get(context.project_scope.as_str())
            .ok_or_else(|| {
                format!(
                    "BINDING_CONTROL_NAME_MISSING: project {} has no name occupancy for {worker_id}",
                    context.project_scope.as_str()
                )
            })?;
        let mut by_worker = names
            .values()
            .filter(|holder| holder.worker_id.as_str() == worker_id);
        let holder = match binding.session_id.as_ref() {
            Some(session) => {
                let matches = by_worker
                    .filter(|holder| holder.session_id == session.as_str())
                    .collect::<Vec<_>>();
                match matches.as_slice() {
                    [holder] => *holder,
                    [] => {
                        return Err(format!(
                            "BINDING_CONTROL_NAME_MISSING: {worker_id} has no stable name for session {session}"
                        ))
                    }
                    _ => {
                        return Err(format!(
                            "BINDING_CONTROL_NAME_AMBIGUOUS: {worker_id} has multiple stable names for session {session}"
                        ))
                    }
                }
            }
            None => match by_worker.collect::<Vec<_>>().as_slice() {
                [holder] => *holder,
                [] => {
                    return Err(format!(
                        "BINDING_CONTROL_NAME_MISSING: {worker_id} has no stable name"
                    ))
                }
                _ => {
                    return Err(format!(
                        "BINDING_CONTROL_NAME_AMBIGUOUS: {worker_id} has multiple stable names"
                    ))
                }
            },
        };
        Ok(crate::server::binding_slot::IdentityBindingKey::new(
            context.project_scope.clone(),
            context.app_scope_id.clone(),
            holder.name.clone(),
        ))
    }

    /// Build the durable handoff intent for one control-first decision. A
    /// no-change carrier keeps no effect. Automatic fixed-name or session
    /// inheritance needs the displaced holder's exact worker/session as
    /// `previous`; the explicit rename carries the caller's previous names plus
    /// the displaced holder of the target name. When no handoff applies, `None`
    /// keeps the committed control slot without a pending effect. Pane data is
    /// retained only as a legacy deserialization field and is never written by
    /// a new decision.
    fn handoff_effect(
        state: &State,
        context: &ProjectContext,
        worker_id: &str,
        target_name: &str,
        target_session_id: &str,
        operation_id: &str,
        effect_id: &str,
        kind: crate::server::binding_slot::PendingEffectKind,
        source_names: Vec<String>,
        moves_existing_names: bool,
    ) -> Option<crate::server::binding_slot::PendingEffect> {
        let target_carrier_is_current = state
            .name_occupancy(context.project_scope.as_str(), target_name)
            .is_some_and(|holder| {
                holder.worker_id == worker_id
                    && holder.session_id == target_session_id
            });
        let has_rename_source = source_names
            .iter()
            .any(|name| name != target_name);
        if target_carrier_is_current && !has_rename_source {
            return None;
        }
        let previous = state.name_occupancy(context.project_scope.as_str(), target_name);
        let changed_previous = previous.is_some_and(|holder| {
            holder.worker_id != worker_id || holder.session_id != target_session_id
        });
        if !changed_previous && !has_rename_source {
            return None;
        }
        let occupancy_revision = previous
            .filter(|holder| {
                holder.worker_id == worker_id && holder.session_id == target_session_id
            })
            .map(|holder| holder.occupancy_revision)
            .unwrap_or_else(|| {
                state
                    .name_occupancy_revision(
                        context.project_scope.as_str(),
                        target_name,
                    )
                    .checked_add(1)
                    .unwrap_or(0)
            });
        Some(crate::server::binding_slot::PendingEffect {
            kind,
            effect_id: effect_id.to_owned(),
            operation_id: operation_id.to_owned(),
            project_scope: context.project_scope.clone(),
            app_scope_id: context.app_scope_id.clone(),
            name: target_name.to_owned(),
            previous_worker_id: previous.map(|holder| holder.worker_id.clone()),
            previous_session_id: previous.map(|holder| holder.session_id.clone()),
            target_worker_id: worker_id.to_owned(),
            target_session_id: target_session_id.to_owned(),
            occupancy_revision,
            source_names,
            moves_existing_names,
            pane_anchor: None,
            created_ms: crate::server::now_ms(),
        })
    }

    /// Resolve the stable name a restored identity already holds, without
    /// inventing one. `None` means the name owner has not decided this
    /// carrier's name yet; that decision happens once later from live facts,
    /// so a refresh must not assign a pool name and preempt it here.
    fn worker_control_name_opt(
        &self,
        runtime: &Server,
        context: &ProjectContext,
        worker_id: &str,
        binding: &RuntimeBinding,
    ) -> anyhow::Result<Option<String>> {
        match self.control_key_for_worker(runtime, context, worker_id, binding) {
            Ok(key) => Ok(Some(key.name)),
            Err(error) if error.starts_with("BINDING_CONTROL_NAME_MISSING") => Ok(None),
            Err(error) => Err(anyhow::Error::msg(error)),
        }
    }

    /// Publish the control slot for a restored identity only when it already
    /// holds a stable name decided by the name owner. A nameless legacy
    /// identity is left for the later single context name decision, which
    /// publishes its own slot from the live pane and cwd facts.
    fn publish_refresh_control_slot(
        &self,
        runtime: &Server,
        context: &ProjectContext,
        worker_id: &str,
        binding: RuntimeBinding,
        transport: SelectedTransport,
    ) -> anyhow::Result<()> {
        let Some(name) = self.worker_control_name_opt(runtime, context, worker_id, &binding)?
        else {
            return Ok(());
        };
        let committed = self
            .commit_control_binding_named(context, &name, binding, transport, None)
            .map_err(anyhow::Error::msg)?;
        if committed.durability == crate::server::binding_store::Durability::Unknown {
            anyhow::bail!("BINDING_CONTROL_DURABILITY_UNKNOWN: control was published but its durability is unknown");
        }
        Ok(())
    }

    /// Commit the control binding under a stable name that the name owner has
    /// already decided from real current facts. Recovery cannot derive the
    /// slot key from a pre-existing occupancy: an old identity may have no name
    /// yet, so the name decision runs first and hands the exact name here.
    fn commit_control_binding_named(
        &self,
        context: &ProjectContext,
        name: &str,
        binding: RuntimeBinding,
        transport: SelectedTransport,
        expected_generation: Option<u64>,
    ) -> Result<crate::server::binding_store::StoreCommit, String> {
        self.commit_control_change(control_commit_change(
            context,
            name,
            binding,
            transport,
            expected_generation,
        ))
    }

    /// Locked form of [`commit_control_binding_named`] for a caller that
    /// already holds the register gate.
    fn commit_control_binding_named_locked(
        &self,
        context: &ProjectContext,
        name: &str,
        binding: RuntimeBinding,
        transport: SelectedTransport,
        expected_generation: Option<u64>,
    ) -> Result<crate::server::binding_store::StoreCommit, String> {
        self.commit_control_change_locked(control_commit_change(
            context,
            name,
            binding,
            transport,
            expected_generation,
        ))
    }

    /// Publish the control slot for the name the single context name decision
    /// just planned. The caller holds the register gate and the project-state
    /// lock, so the target name comes from the same snapshot as the business
    /// handoff that follows. A no-op repeat (same carrier and session keeping
    /// its name) is idempotent and never rotates the slot generation.
    pub(crate) fn commit_control_name_decision_locked(
        &self,
        state: &State,
        context: &ProjectContext,
        worker_id: &str,
        target_name: &str,
        operation_id: &str,
        effect_id: &str,
    ) -> Result<crate::server::binding_store::Durability, String> {
        let (binding, transport) =
            Self::control_binding_and_transport(state, context, worker_id, "name decision")?;
        let key = crate::server::binding_slot::IdentityBindingKey::new(
            context.project_scope.clone(),
            context.app_scope_id.clone(),
            target_name.to_owned(),
        );
        let target_session_id = binding
            .session_id
            .as_ref()
            .ok_or_else(|| {
                "BINDING_CONTROL_TRANSPORT_MISSING: name decision binding has no agent session"
                    .to_owned()
            })?
            .as_str()
            .to_owned();
        let value = crate::server::binding_slot::BindingValue::new(binding, transport);
        match Self::handoff_effect(
            state,
            context,
            worker_id,
            target_name,
            &target_session_id,
            operation_id,
            effect_id,
            crate::server::binding_slot::PendingEffectKind::Name,
            Vec::new(),
            false,
        ) {
            Some(effect) => self
                .commit_control_change_locked(
                    crate::server::binding_store::ControlChange::CommitWithEffect {
                        key,
                        value,
                        expected_generation: None,
                        effect,
                    },
                )
                .map(|commit| commit.durability),
            None => self
                .commit_control_change_locked(
                    crate::server::binding_store::ControlChange::Commit {
                        key,
                        value,
                        expected_generation: None,
                    },
                )
                .map(|commit| commit.durability),
        }
    }

    /// Publish the exact prepared Register binding into the control resource
    /// before the business Register commit. The prepared envelope is the same
    /// snapshot whose binding and transport the reducer then commits, so the
    /// control slot and the business occupancy stay on one candidate. The name
    /// owner is consulted read-only with the candidate binding facts; nothing
    /// is scanned from a journal and no name is invented when the planner
    /// cannot decide.
    pub(crate) fn publish_register_control_before_consume(
        &self,
        runtime: &Server,
        prepared: &crate::server::PreparedRegisterEnvelope,
        tmux_session_name: Option<&str>,
    ) -> Result<String, String> {
        let crate::server::state::TypedCommand::RegisterWorker {
            binding, worker, ..
        } = &prepared.typed.command
        else {
            return Err(
                "BINDING_CONTROL_REGISTER_REJECTED: prepared envelope is not a RegisterWorker command"
                    .into(),
            );
        };
        let Some(transport) = worker.transport.clone() else {
            return Err(
                "BINDING_CONTROL_TRANSPORT_MISSING: prepared Register worker has no selected transport"
                    .into(),
            );
        };
        let context = crate::proto::ProjectContext {
            app_scope_id: binding.app_scope_id.clone(),
            canonical_root: runtime.root.to_string_lossy().into_owned(),
            project_scope: binding.project_scope.clone(),
            runtime_context: None,
        };
        let _register = self.register_gate.lock().unwrap();
        let state = runtime.state.lock().unwrap();
        let Some(name) = name_resolution::plan_automatic_name_for_binding(
            &state,
            &prepared.nested_operation_id,
            "register",
            binding,
            tmux_session_name,
        )?
        else {
            return Err(
                "NAME_PENDING: register control-first name decision requires authorization".into(),
            );
        };
        let target_session_id = binding
            .session_id
            .as_ref()
            .ok_or(
                "BINDING_CONTROL_REGISTER_REJECTED: candidate binding has no agent session",
            )?
            .as_str()
            .to_owned();
        let effect_id = format!("register-{}", prepared.nested_operation_id);
        let effect = Self::handoff_effect(
            &state,
            &context,
            binding.agent_id.as_str(),
            &name,
            &target_session_id,
            &prepared.nested_operation_id,
            &effect_id,
            crate::server::binding_slot::PendingEffectKind::Register,
            Vec::new(),
            false,
        );
        let key = crate::server::binding_slot::IdentityBindingKey::new(
            context.project_scope.clone(),
            context.app_scope_id.clone(),
            name.clone(),
        );
        let value =
            crate::server::binding_slot::BindingValue::new(binding.clone(), transport);
        let committed = match effect {
            Some(effect) => self.commit_control_change_locked(
                crate::server::binding_store::ControlChange::CommitWithEffect {
                    key,
                    value,
                    expected_generation: None,
                    effect,
                },
            )?,
            None => self.commit_control_change_locked(
                crate::server::binding_store::ControlChange::Commit {
                    key,
                    value,
                    expected_generation: None,
                },
            )?,
        };
        if committed.durability == crate::server::binding_store::Durability::Unknown {
            return Err(
                "BINDING_CONTROL_DURABILITY_UNKNOWN: the register control slot was published but \
                 its parent directory sync is unconfirmed; do not report this registration as a \
                 plain success"
                    .into(),
            );
        }
        Ok(effect_id)
    }

    /// Clear the exact control-first Register effect after the business
    /// registration and its route publication succeeded. A failed dispatch
    /// leaves the effect in place so the next recovery pass can either apply
    /// the handoff or report the missing business owner.
    pub(crate) fn clear_register_pending_effect(
        &self,
        effect_id: Option<&str>,
    ) -> Result<(), String> {
        let Some(effect_id) = effect_id else {
            return Ok(());
        };
        self.clear_pending_effect(effect_id)
    }

    /// Clear one completed pending effect. The caller must not hold the
    /// register gate; this helper acquires it once for the control publish.
    pub(crate) fn clear_pending_effect(&self, effect_id: &str) -> Result<(), String> {
        let _register = self.register_gate.lock().unwrap();
        self.clear_pending_effect_locked(effect_id)
    }

    /// Clear one completed pending effect while the caller already holds the
    /// register gate. The name-decision critical section takes the gate once
    /// and runs plan, control publish, business commit and effect clear under
    /// that single lock.
    pub(crate) fn clear_pending_effect_locked(&self, effect_id: &str) -> Result<(), String> {
        let committed = self.commit_control_change_locked(
            crate::server::binding_store::ControlChange::ClearEffect {
                effect_id: effect_id.to_owned(),
            },
        )?;
        match committed.durability {
            crate::server::binding_store::Durability::Confirmed => Ok(()),
            crate::server::binding_store::Durability::Unknown => Err(
                "BINDING_CONTROL_DURABILITY_UNKNOWN: effect clear was published but its parent \
                 directory sync is unconfirmed"
                    .into(),
            ),
        }
    }

    /// Consume every durable control effect before the host publishes a
    /// current route. A completed business handoff only clears its effect. An
    /// incomplete one is replayed from the exact before/after carrier and
    /// source names stored in the control file, never inferred from a journal.
    pub(crate) fn recover_pending_effects(&self) -> Result<(), String> {
        let _register = self.register_gate.lock().unwrap();
        // Reload under the gate. Startup passes through here once, and the
        // request admission path may run concurrently with a completed
        // handoff. A stale snapshot must not replay an effect that another
        // writer already consumed.
        let loaded = self.binding_control_store().load()?;
        let effects: Vec<_> = loaded.pending_effects().cloned().collect();
        let mut first_error: Option<String> = None;
        for effect in effects {
            if let Err(error) = self.recover_pending_effect_locked(&loaded, &effect) {
                // One unfinished business command must not stop the whole
                // daemon. The effect stays durable, and the per-scope
                // admission gate keeps dependent work blocked until a real
                // Register/context converges it. Only a corrupt control
                // resource fails closed at load().
                append_log(
                    &self.host.log_path(),
                    &format!("PENDING_EFFECT_RECOVERY_BLOCKED: {error}"),
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Consume the effects belonging to one context scope. This is the runtime
    /// admission path for a daemon that stayed up after a failed business
    /// commit; startup uses the same recovery function without a client call.
    pub(crate) fn recover_pending_effects_for_scope(
        &self,
        context: &ProjectContext,
    ) -> Result<(), String> {
        let store = self.binding_control_store();
        let loaded = store.load()?;
        if loaded
            .pending_effect_for_scope(&context.project_scope, &context.app_scope_id)
            .next()
            .is_none()
        {
            return Ok(());
        }
        let _register = self.register_gate.lock().unwrap();
        // Reload under the gate so a concurrently completed effect is not
        // applied twice from a stale snapshot.
        let loaded = self.binding_control_store().load()?;
        let effects: Vec<_> = loaded
            .pending_effect_for_scope(&context.project_scope, &context.app_scope_id)
            .cloned()
            .collect();
        let mut first_error: Option<String> = None;
        for effect in effects {
            if let Err(error) = self.recover_pending_effect_locked(&loaded, &effect) {
                append_log(
                    &self.host.log_path(),
                    &format!("PENDING_EFFECT_SCOPE_BLOCKED: {error}"),
                );
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn recover_pending_effect_locked(
        &self,
        control: &crate::server::binding_slot::BindingControlSet,
        effect: &crate::server::binding_slot::PendingEffect,
    ) -> Result<(), String> {
        // A newer control commit for the same stable name supersedes this
        // intent. Replaying or blocking on the older carrier would regress the
        // name or block a scope that already converged; the effect is retired
        // so the next Register/context pass owns the name again.
        if Self::pending_effect_superseded(control, effect) {
            append_log(
                &self.host.log_path(),
                &format!(
                    "PENDING_EFFECT_SUPERSEDED_CLEARED: effect {} for {} ({}) no longer owns the current control intent",
                    effect.effect_id, effect.name, effect.target_worker_id
                ),
            );
            let committed = self.commit_control_change_locked(
                crate::server::binding_store::ControlChange::ClearEffect {
                    effect_id: effect.effect_id.clone(),
                },
            )?;
            return match committed.durability {
                crate::server::binding_store::Durability::Confirmed => Ok(()),
                crate::server::binding_store::Durability::Unknown => Err(format!(
                    "BINDING_CONTROL_DURABILITY_UNKNOWN: superseded pending effect {} was retired but its clear publication is not directory-confirmed",
                    effect.effect_id
                )),
            };
        }
        let key = (
            effect.app_scope_id.as_str().to_owned(),
            effect.project_scope.as_str().to_owned(),
        );
        let runtime = self
            .routes
            .lock()
            .unwrap()
            .get(&key)
            .and_then(|route| route.runtime.clone());
        let Some(runtime) = runtime else {
            return Err(format!(
                "PENDING_EFFECT_RUNTIME_MISSING: effect {} for route {key:?} is not materialized",
                effect.effect_id
            ));
        };
        let route_scope = RouteScope {
            app_scope_id: effect.app_scope_id.clone(),
            project_scope_id: effect.project_scope.clone(),
        };
        let mut state = runtime.state.lock().unwrap();
        if !state
            .workers
            .contains_key(&effect.target_worker_id)
        {
            return Err(format!(
                "PENDING_EFFECT_TARGET_MISSING: effect {} targets worker {} with no committed registration",
                effect.effect_id, effect.target_worker_id
            ));
        }
        if effect.kind == crate::server::binding_slot::PendingEffectKind::Register
            && !Self::pending_effect_complete(&state, effect)
        {
            // A Register effect is published before the business Register
            // command. If that command never landed, the reducer has no
            // worker/token/name occupancy to replay. Synthesizing one from the
            // control slot would invent a carrier and bypass the Register
            // owner. Keep the exact effect visible and block dependent
            // business work until a real Register completes it.
            return Err(format!(
                "PENDING_EFFECT_REGISTER_INCOMPLETE: effect {} has no completed business Register",
                effect.effect_id
            ));
        }
        if effect.kind == crate::server::binding_slot::PendingEffectKind::Name
            && !Self::pending_effect_complete(&state, effect)
        {
            let occupancy = crate::server::name_state::NameOccupancy {
                name: effect.name.clone(),
                worker_id: effect.target_worker_id.clone(),
                session_id: effect.target_session_id.clone(),
                occupancy_revision: effect.occupancy_revision,
                updated_ms: effect.created_ms,
                pane_anchor: effect.pane_anchor.clone(),
            };
            let rename_sources = if effect.moves_existing_names {
                effect.source_names.as_slice()
            } else {
                &[]
            };
            let mut events =
                crate::server::name_resolution::build_name_resolution_events_with_rename_from(
                    &state,
                    effect.project_scope.as_str(),
                    &occupancy,
                    rename_sources,
                );
            // The mailbox/subscription/task constructors can legitimately
            // emit no name-resolution record when the target occupancy is
            // already current but an old source name still exists. The
            // occupancy event is the durable owner that releases those source
            // names, so keep it explicit in that case.
            if !events
                .iter()
                .any(|event| matches!(event, Event::NameResolutionCommitted { .. }))
            {
                events.insert(
                    0,
                    Event::NameResolutionCommitted {
                        project_scope: effect.project_scope.as_str().to_owned(),
                        occupancy: occupancy.clone(),
                        assignments: Vec::new(),
                    },
                );
            }
            if effect.name != "Jenny"
                && current_master_worker_id(&state, Some(&route_scope)).as_deref()
                    == Some(effect.target_worker_id.as_str())
            {
                events.extend(master_authority_revoke_events(&state, &route_scope));
            }
            if !events.is_empty() {
                runtime
                    .commit_locked_checked(&mut state, &events)
                    .map_err(|error| format!("PENDING_EFFECT_COMMIT_FAILED: {error}"))?;
            }
        }
        drop(state);
        let committed = self.commit_control_change_locked(
            crate::server::binding_store::ControlChange::ClearEffect {
                effect_id: effect.effect_id.clone(),
            },
        )?;
        match committed.durability {
            crate::server::binding_store::Durability::Confirmed => Ok(()),
            crate::server::binding_store::Durability::Unknown => Err(format!(
                "BINDING_CONTROL_DURABILITY_UNKNOWN: pending effect {} was consumed but its clear \
                 publication is not directory-confirmed",
                effect.effect_id
            )),
        }
    }

    /// Whether the control slot for the effect's stable name no longer points
    /// at the effect's target carrier. A slot that is active on a different
    /// worker/session, or a slot that has been retired, means a newer
    /// registration or handoff replaced this intent. A completely absent slot
    /// is an inconsistent control set and is kept explicit rather than
    /// silently retired.
    fn pending_effect_superseded(
        control: &crate::server::binding_slot::BindingControlSet,
        effect: &crate::server::binding_slot::PendingEffect,
    ) -> bool {
        let key = crate::server::binding_slot::IdentityBindingKey::new(
            effect.project_scope.clone(),
            effect.app_scope_id.clone(),
            effect.name.clone(),
        );
        match control.slot(&key) {
            Some(slot) => match slot.active_binding() {
                Some(value) => {
                    value.binding.agent_id.as_str() != effect.target_worker_id
                        || value.binding.session_id.as_ref().map(|session| session.as_str())
                            != Some(effect.target_session_id.as_str())
                }
                None => true,
            },
            None => false,
        }
    }

    fn pending_effect_complete(
        state: &State,
        effect: &crate::server::binding_slot::PendingEffect,
    ) -> bool {
        let Some(holder) = state.name_occupancy(
            effect.project_scope.as_str(),
            effect.name.as_str(),
        ) else {
            return false;
        };
        if holder.worker_id != effect.target_worker_id
            || holder.session_id != effect.target_session_id
            || holder.occupancy_revision < effect.occupancy_revision
        {
            return false;
        }
        effect
            .source_names
            .iter()
            .filter(|name| *name != &effect.name)
            .all(|name| {
                state
                    .name_occupancy(effect.project_scope.as_str(), name)
                    .is_none_or(|source| {
                        source.worker_id != effect.target_worker_id
                            || source.session_id != effect.target_session_id
                    })
            })
    }

    /// Return a hook the register dispatch can run just before the business
    /// Register consume. It captures the manager and the selected runtime so
    /// the wire path can publish control first without threading the manager
    /// through the free register owner.
    fn register_control_first_hook<'a>(
        &'a self,
        runtime: &'a Arc<Server>,
        tmux_session_name: Option<String>,
        effect_id_out: &'a mut Option<String>,
    ) -> impl FnMut(&crate::server::PreparedRegisterEnvelope) -> Result<(), String> + 'a {
        move |prepared| {
            *effect_id_out = Some(self.publish_register_control_before_consume(
                runtime,
                prepared,
                tmux_session_name.as_deref(),
            )?);
            Ok(())
        }
    }

    /// Publish the successful Register owner's exact reducer binding into the
    /// host control resource. The stable name is already decided by the
    /// adjacent name owner, so this does not choose, inherit or rewrite it.
    pub(crate) fn persist_registered_binding(
        &self,
        runtime: &Server,
        context: &ProjectContext,
        worker_id: &str,
    ) -> Result<crate::server::binding_store::Durability, String> {
        let (binding, transport) = {
            let state = runtime.state.lock().unwrap();
            Self::control_binding_and_transport(
                &state,
                context,
                worker_id,
                "successful registration",
            )?
        };
        // Decide the stable name before publishing the control slot. The name
        // owner is the real current-facts authority; a control commit cannot
        // require a pre-existing occupancy because a legacy identity may have
        // no name yet. Registration keeps its exact user contract: same session
        // keeps the name, otherwise the pane anchor or fixed pool decides.
        let name = name_resolution::ensure_worker_control_name(runtime, worker_id)?;
        self.commit_control_binding_named(
            context,
            &name,
            binding,
            transport,
            None,
        )
        .map(|commit| commit.durability)
    }

    /// Commit an explicit ordinary name move to the binding-control resource
    /// before the business name/handoff commit runs. The caller holds the
    /// register gate and the project-state lock. The target stable name
    /// receives the caller's exact current binding, and every previous name the
    /// caller held is retired in the same atomic control write. Jenny takeovers
    /// and pending decisions return `None` from the planner and stay on the
    /// existing grant/confirmation path. Returns the moved target name when a
    /// control move was published.
    fn commit_explicit_name_control_move_locked(
        &self,
        runtime: &Server,
        state: &State,
        context: &ProjectContext,
        worker_id: &str,
        operation_id: &str,
        invocation: &str,
        intent: &name_resolution::NameIntent,
        tmux_session_name: Option<&str>,
        effect_id: &str,
    ) -> Result<Option<String>, String> {
        let Some(planned) = name_resolution::plan_explicit_name_move_locked(
            runtime,
            state,
            worker_id,
            operation_id,
            invocation,
            intent,
            tmux_session_name,
        )?
        else {
            return Ok(None);
        };
        let (binding, transport) =
            Self::control_binding_and_transport(state, context, worker_id, "explicit name move")?;
        let target = crate::server::binding_slot::IdentityBindingKey::new(
            context.project_scope.clone(),
            context.app_scope_id.clone(),
            planned.target.clone(),
        );
        let target_session_id = binding
            .session_id
            .as_ref()
            .ok_or_else(|| {
                "BINDING_CONTROL_TRANSPORT_MISSING: explicit name move binding has no agent session"
                    .to_owned()
            })?
            .as_str()
            .to_owned();
        let effect = Self::handoff_effect(
            state,
            context,
            worker_id,
            &planned.target,
            &target_session_id,
            operation_id,
            effect_id,
            crate::server::binding_slot::PendingEffectKind::Name,
            planned.source_names.clone(),
            intent.moves_existing_names,
        );
        let commit = match effect {
            Some(effect) => self.commit_control_change_locked(
                crate::server::binding_store::ControlChange::MoveNameWithEffect {
                    target,
                    value: crate::server::binding_slot::BindingValue::new(binding, transport),
                    source_names: planned.source_names,
                    expected_generation: None,
                    effect,
                },
            )?,
            None => self.commit_control_change_locked(
                crate::server::binding_store::ControlChange::MoveName {
                    target,
                    value: crate::server::binding_slot::BindingValue::new(binding, transport),
                    source_names: planned.source_names,
                    expected_generation: None,
                },
            )?,
        };
        match commit.durability {
            crate::server::binding_store::Durability::Confirmed => Ok(Some(planned.target)),
            crate::server::binding_store::Durability::Unknown => Err(
                "BINDING_CONTROL_DURABILITY_UNKNOWN: the explicit name move was published but its \
                 parent directory sync is unconfirmed; do not replay it as a success"
                    .into(),
            ),
        }
    }

    pub(crate) fn unregister_control_binding(
        &self,
        runtime: &Server,
        context: &ProjectContext,
        worker_id: &str,
        binding: &RuntimeBinding,
    ) -> Result<crate::server::binding_store::Durability, String> {
        let key = self.control_key_for_worker(runtime, context, worker_id, binding)?;
        self.commit_control_change(crate::server::binding_store::ControlChange::Unregister {
            key,
            expected_generation: None,
        })
        .map(|commit| commit.durability)
    }

    /// A published control file whose parent directory could not be synced is
    /// visible but not known to be durable. It must never be reported as a
    /// plain success: the caller can neither treat the write as confirmed nor
    /// replay it as if nothing landed.
    fn mark_control_durability_unknown(response: &mut Resp) {
        response.ok = false;
        response.error = Some(
            "BINDING_CONTROL_DURABILITY_UNKNOWN: the binding control file was published \
             but its parent directory sync result is unknown; do not replay this as a success"
                .into(),
        );
        if !response.data.is_object() {
            response.data = json!({});
        }
        if let Some(object) = response.data.as_object_mut() {
            object.insert("control_committed".into(), json!(true));
            object.insert("durability".into(), json!("unknown"));
        }
    }

    /// Replay selected control values into the host address index. This is a
    /// control-driven publication, not a scan of project journals for a
    /// current identity. A missing runtime or reducer binding stays explicit
    /// and never falls back to a different durable candidate.
    fn reconcile_controlled_thread_routes(
        &self,
        controlled: &crate::server::binding_slot::BindingControlSet,
    ) -> Result<(), String> {
        for value in controlled.active_values() {
            let key = (
                value.binding.app_scope_id.as_str().to_owned(),
                value.binding.project_scope.as_str().to_owned(),
            );
            let runtime = self
                .routes
                .lock()
                .unwrap()
                .get(&key)
                .and_then(|route| route.runtime.clone());
            let Some(runtime) = runtime else {
                append_log(
                    &self.host.log_path(),
                    &format!(
                        "BINDING_CONTROL_RUNTIME_MISSING: control binding {} for route {key:?} is not materialized",
                        value.binding.binding_id
                    ),
                );
                continue;
            };
            let exact = runtime
                .state
                .lock()
                .unwrap()
                .global
                .lookup_binding_for(&value.binding.route_scope(), &value.binding.binding_id)
                .is_some_and(|binding| binding == &value.binding);
            if !exact {
                append_log(
                    &self.host.log_path(),
                    &format!(
                        "BINDING_CONTROL_RUNTIME_CONFLICT: control binding {} for route {key:?} is missing from its runtime",
                        value.binding.binding_id
                    ),
                );
                continue;
            }
            self.reconcile_binding_publication(&value.binding)?;
        }
        Ok(())
    }
}

/// Build the `Commit` control change for one decided stable name from the
/// exact current binding and transport. Both the locking and locked control
/// entries call this so the key and value stay identical.
fn control_commit_change(
    context: &ProjectContext,
    name: &str,
    binding: RuntimeBinding,
    transport: SelectedTransport,
    expected_generation: Option<u64>,
) -> crate::server::binding_store::ControlChange {
    let key = crate::server::binding_slot::IdentityBindingKey::new(
        context.project_scope.clone(),
        context.app_scope_id.clone(),
        name.to_owned(),
    );
    crate::server::binding_store::ControlChange::Commit {
        key,
        value: crate::server::binding_slot::BindingValue::new(binding, transport),
        expected_generation,
    }
}

#[cfg(test)]
pub(crate) mod test_hooks {
    thread_local! {
        static FAIL_NEXT_PARENT_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    pub(crate) fn fail_next_parent_sync() {
        FAIL_NEXT_PARENT_SYNC.with(|flag| flag.set(true));
    }

    pub(crate) fn take_fail_next_parent_sync() -> bool {
        FAIL_NEXT_PARENT_SYNC.with(|flag| flag.replace(false))
    }
}
