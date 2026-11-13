impl ProjectRuntimeManager {
    fn ensure_current_worker_for_context(
        &self,
        context: &ProjectContext,
        facts: &crate::proto::IdentityFacts,
        invocation_cwd: Option<&str>,
    ) -> Result<(), String> {
        let control = self.binding_control_store().load()?;
        if control
            .lookup(
                &context.project_scope,
                &context.app_scope_id,
                facts,
                invocation_cwd,
            )?
            .is_none()
        {
            return Ok(());
        }
        let root = Path::new(&context.canonical_root);
        context
            .validate_registered_root(root)
            .map_err(|error| format!("{error:#}"))?;
        validate_project_registration_cwd(&context.canonical_root, root)?;
        let runtime = self.select_runtime(context)?;
        let _register = self.register_gate.lock().unwrap();
        let control = self.binding_control_store().load()?;
        let Some(slot) = control.lookup(
            &context.project_scope,
            &context.app_scope_id,
            facts,
            invocation_cwd,
        )?
        else {
            return Ok(());
        };
        let value = slot
            .active_binding()
            .expect("lookup selects active current");
        let pending = control.pending_effects().find(|effect| {
            effect.kind == crate::server::binding_slot::PendingEffectKind::Register
                && effect.project_scope == context.project_scope
                && effect.app_scope_id == context.app_scope_id
                && effect.name == slot.identity_key.name
                && effect.target_worker_id == value.binding.agent_id.as_str()
                && value.binding.session_id.as_ref().map(|id| id.as_str())
                    == Some(effect.target_session_id.as_str())
                && !Self::pending_effect_superseded(&control, effect)
        });
        self.materialize_current_worker_locked(&runtime, value, pending)
    }

    /// The current map already owns this principal. Recreate only its missing
    /// business relationship through the same atomic Register transaction.
    fn materialize_current_worker_locked(
        &self,
        runtime: &Server,
        value: &crate::server::binding_slot::BindingValue,
        pending: Option<&crate::server::binding_slot::PendingEffect>,
    ) -> Result<(), String> {
        let worker = value.binding.agent_id.as_str();
        if runtime.state.lock().unwrap().workers.contains_key(worker) {
            return Ok(());
        }
        let credential = crate::identity::read_persisted(&self.host.host_paths, worker)
            .map_err(|error| format!("{error:#}"))?
            .ok_or("IDENTITY_CREDENTIAL_MISSING: mapped worker has no credential")?;
        let principal = credential
            .runtime
            .as_ref()
            .ok_or("IDENTITY_CREDENTIAL_INVALID: mapped credential has no registered principal")?;
        if credential.worker_id != worker
            || credential.project_scope.as_ref() != Some(&value.binding.project_scope)
            || principal.agent_id != value.binding.agent_id
            || principal.appserver_id != value.binding.app_scope_id
            || credential.token.trim().is_empty()
        {
            return Err(
                "IDENTITY_CREDENTIAL_INVALID: exact mapped principal or token differs".into(),
            );
        }
        let prepared = runtime.prepare_register_envelope_for_current(
            worker,
            &credential.token,
            &value.transport,
            value.binding.project_scope.clone(),
            &runtime.root.to_string_lossy(),
            value.binding.app_scope_id.clone(),
            false,
            Some(&value.binding),
            pending
                .filter(|effect| {
                    effect.kind == crate::server::binding_slot::PendingEffectKind::Register
                })
                .map(|effect| effect.operation_id.as_str()),
        )?;
        let origin = pending
            .filter(|effect| {
                effect.kind == crate::server::binding_slot::PendingEffectKind::Register
                    && effect.operation_id == prepared.nested_operation_id
            })
            .map(|effect| RegisterOrigin::InitialCurrentContinuation {
                explicit_peer: effect.name != "Jenny",
            })
            .unwrap_or(RegisterOrigin::CurrentPrincipal);
        runtime
            .typed_dispatch_register_with_origin(prepared.typed, None, origin)
            .map_err(|error| format!("CURRENT_WORKER_MATERIALIZATION_FAILED: {error}"))?;
        if !runtime.state.lock().unwrap().workers.contains_key(worker) {
            return Err(
                "CURRENT_WORKER_MATERIALIZATION_INCOMPLETE: Register receipt has no worker".into(),
            );
        }
        Ok(())
    }

    pub(crate) fn consume_register_effect(&self, effect_id: Option<&str>) -> Result<(), String> {
        let effect_id = effect_id.ok_or("REGISTER_CONTROL_INTENT_REQUIRED")?;
        let _register = self.register_gate.lock().unwrap();
        let map = self.binding_control_store().load()?;
        if let Some(effect) = map.pending_effect(effect_id) {
            self.recover_pending_effect_locked(&map, effect)?;
        }
        Ok(())
    }

    fn retire_mapped_self(
        &self,
        runtime: &Server,
        context: &ProjectContext,
        worker: &str,
        token: &str,
    ) -> Resp {
        use crate::server::binding_slot::{PendingEffect, PendingEffectKind};
        let _register = self.register_gate.lock().unwrap();
        {
            let state = runtime.state.lock().unwrap();
            if let Err(error) = verify(&state, worker, token) {
                return error;
            }
        }
        let map = match self.binding_control_store().load() {
            Ok(map) => map,
            Err(error) => return Resp::err(error),
        };
        let key = match map.key_for_worker(&context.project_scope, &context.app_scope_id, worker) {
            Ok(Some(key)) => key.clone(),
            Ok(None) => return Resp::data(json!({"result":{"outcome":"complete"}})),
            Err(error) => return Resp::err(error),
        };
        let value = map
            .slot(&key)
            .and_then(|slot| slot.active_binding())
            .expect("active key");
        let Some(session) = value.binding.session_id.as_ref() else {
            return Resp::err("IDENTITY_SESSION_MISSING");
        };
        let effect = PendingEffect {
            kind: PendingEffectKind::Retire,
            role: None,
            effect_id: format!("retire-{}-{}", value.binding.binding_id, now_ms()),
            operation_id: format!("self-unregister-{}", value.binding.binding_id),
            project_scope: key.project_scope.clone(),
            app_scope_id: key.app_scope_id.clone(),
            name: key.name.clone(),
            previous_worker_id: Some(worker.into()),
            previous_session_id: Some(session.to_string()),
            target_worker_id: worker.into(),
            target_session_id: session.to_string(),
            occupancy_revision: 1,
            source_names: Vec::new(),
            moves_existing_names: false,
            pane_anchor: None,
            created_ms: now_ms(),
        };
        match self.commit_control_change_locked(
            crate::server::binding_store::ControlChange::UnregisterWithEffect {
                key,
                effect: effect.clone(),
            },
        ) {
            Ok(commit)
                if commit.durability == crate::server::binding_store::Durability::Confirmed => {}
            Ok(_) => return Resp::err_data(
                "BINDING_CONTROL_DURABILITY_UNKNOWN: retirement published; effects remain pending",
                json!({"control_committed":true}),
            ),
            Err(error) => return Resp::err(error),
        }
        let published = match self.binding_control_store().load() {
            Ok(map) => map,
            Err(error) => return Resp::err(error),
        };
        match self.recover_pending_effect_locked(&published, &effect) {
            Ok(()) => Resp::data(json!({"result":{"outcome":"complete"}})),
            Err(error) => Resp::err_data(
                format!("CONTROL_COMMITTED_EFFECTS_PENDING: {error}"),
                json!({"control_committed":true}),
            ),
        }
    }

    fn mapped_key_for_worker(
        &self,
        context: &ProjectContext,
        worker: &str,
        _binding: &RuntimeBinding,
    ) -> Result<crate::server::binding_slot::IdentityBindingKey, String> {
        self.binding_control_store()
            .load()?
            .key_for_worker(&context.project_scope, &context.app_scope_id, worker)?
            .cloned()
            .ok_or_else(|| "IDENTITY_MAP_NOT_FOUND: worker has no current name".into())
    }

    fn mapped_adapter_bindings(
        &self,
        runtime: &Arc<Server>,
        kind: &TransportKind,
    ) -> Vec<RuntimeBinding> {
        let control = match self.binding_control_store().load() {
            Ok(control) => control,
            Err(error) => {
                append_log(&self.host.log_path(), &error);
                return Vec::new();
            }
        };
        control
            .active_values()
            .filter(|value| {
                &value.transport.kind == kind
                    && value.binding.native_thread_id.is_some()
                    && Path::new(value.binding.project_scope.as_str()) == runtime.root
            })
            .map(|value| value.binding.clone())
            .collect()
    }

    fn binding_control_store(&self) -> crate::server::binding_store::BindingControlStore {
        crate::server::binding_store::BindingControlStore::new(self.host.host_paths.state_root())
    }

    pub(crate) fn commit_control_change(
        &self,
        change: crate::server::binding_store::ControlChange,
    ) -> Result<crate::server::binding_store::StoreCommit, String> {
        let _register = self.register_gate.lock().unwrap();
        self.commit_control_change_locked(change)
    }

    fn prepare_control_value(
        control: &crate::server::binding_slot::BindingControlSet,
        key: &crate::server::binding_slot::IdentityBindingKey,
        value: &mut crate::server::binding_slot::BindingValue,
    ) -> Result<(), String> {
        let previous = control.active_values().find(|saved| {
            saved.binding.agent_id == value.binding.agent_id
                && saved.binding.route_scope() == value.binding.route_scope()
        });
        if let Some(previous) = previous {
            value.tab_id = previous.tab_id.clone();
            value.layout_binding_id = previous.layout_binding_id.clone();
            if value.host.agent_kind.is_none() {
                value.host.agent_kind = previous.host.agent_kind.clone();
            }
            if value.host.profile.is_none() {
                value.host.profile = previous.host.profile.clone();
            }
            if value.invocation_cwd == key.project_scope.as_str() {
                value.invocation_cwd = previous.invocation_cwd.clone();
            }
        }
        if let Some(endpoint) = value.binding.tmux_endpoint.as_ref() {
            value.physical_name = Some(crate::client::adapters::tmux::observe_session_name(
                endpoint,
            )?);
            if let Ok(Some(layout)) = crate::server::tm_layout::observe(value) {
                value.layout_binding_id = Some(layout.binding_id);
            }
            value.host.host_type = match value.host.agent_kind.as_deref() {
                Some("codex") => "codex_tui",
                Some("antigravity") => "agy_tui",
                _ => "tui",
            }
            .into();
        }
        Ok(())
    }

    /// Commit one control change while the caller already holds the register
    /// gate. The name-decision critical section takes the gate once and then
    /// runs plan, control publish and business commit under a single
    /// project-state lock; this lock-free entry keeps that sequence from
    /// recursively acquiring the non-reentrant gate.
    pub(crate) fn commit_control_change_locked(
        &self,
        mut change: crate::server::binding_store::ControlChange,
    ) -> Result<crate::server::binding_store::StoreCommit, String> {
        use crate::server::binding_store::ControlChange;
        let control = self.binding_control_store().load()?;
        let value_and_key = match &mut change {
            ControlChange::Commit { key, value, .. }
            | ControlChange::CommitWithEffect { key, value, .. } => Some((key, value)),
            ControlChange::MoveName { target, value, .. }
            | ControlChange::MoveNameWithEffect { target, value, .. } => Some((target, value)),
            _ => None,
        };
        if let Some((key, value)) = value_and_key {
            Self::prepare_control_value(&control, key, value)?;
        }
        let change = match change {
            ControlChange::Commit {
                key,
                value,
                expected_generation,
            } => {
                let sources = control
                    .slots()
                    .filter(|slot| {
                        slot.identity_key != key
                            && slot.identity_key.project_scope == key.project_scope
                            && slot.identity_key.app_scope_id == key.app_scope_id
                            && slot.active_binding().is_some_and(|saved| {
                                saved.binding.agent_id == value.binding.agent_id
                            })
                    })
                    .map(|slot| slot.identity_key.name.clone())
                    .collect();
                ControlChange::MoveName {
                    target: key,
                    value,
                    source_names: sources,
                    expected_generation,
                }
            }
            ControlChange::CommitWithEffect {
                key,
                value,
                expected_generation,
                effect,
            } => {
                let sources = control
                    .slots()
                    .filter(|slot| {
                        slot.identity_key != key
                            && slot.identity_key.project_scope == key.project_scope
                            && slot.identity_key.app_scope_id == key.app_scope_id
                            && slot.active_binding().is_some_and(|saved| {
                                saved.binding.agent_id == value.binding.agent_id
                            })
                    })
                    .map(|slot| slot.identity_key.name.clone())
                    .collect();
                ControlChange::MoveNameWithEffect {
                    target: key,
                    value,
                    source_names: sources,
                    expected_generation,
                    effect,
                    companion: None,
                }
            }
            change => change,
        };
        let store = self.binding_control_store();
        #[cfg(test)]
        if test_hooks::take_fail_next_parent_sync() {
            store.fail_parent_sync_on_next_publish();
        }
        store.commit(change)
    }

    /// Build the durable handoff intent for one control-first decision. A
    /// no-change carrier keeps no effect. Automatic fixed-name or session
    /// inheritance needs the displaced holder's exact worker/session as
    /// `previous`; the explicit rename carries the caller's previous names plus
    /// the displaced holder of the target name. When no handoff applies, `None`
    /// keeps the committed control slot without a pending effect. The
    /// daemon-observed pane and invocation cwd travel with the effect so the
    /// replayed business occupancy keeps the same joint recovery anchor.
    fn handoff_effect(
        control: &crate::server::binding_slot::BindingControlSet,
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
        pane_anchor: Option<crate::server::name_state::NamePaneAnchor>,
    ) -> Option<crate::server::binding_slot::PendingEffect> {
        let target_carrier_is_current = state
            .name_occupancy(context.project_scope.as_str(), target_name)
            .is_some_and(|holder| {
                holder.worker_id == worker_id && holder.session_id == target_session_id
            });
        let has_rename_source = source_names.iter().any(|name| name != target_name);
        if target_carrier_is_current && !has_rename_source {
            return None;
        }
        let key = crate::server::binding_slot::IdentityBindingKey::new(
            context.project_scope.clone(),
            context.app_scope_id.clone(),
            target_name,
        );
        let previous = state
            .name_occupancy(context.project_scope.as_str(), target_name)
            .filter(|holder| {
                control
                    .slot(&key)
                    .and_then(|slot| slot.active_binding())
                    .is_some_and(|value| {
                        value.binding.agent_id.as_str() == holder.worker_id
                            && value.binding.session_id.as_ref().map(|id| id.as_str())
                                == Some(holder.session_id.as_str())
                    })
            });
        let changed_previous = previous.is_some_and(|holder| {
            holder.worker_id != worker_id || holder.session_id != target_session_id
        });
        if kind != crate::server::binding_slot::PendingEffectKind::Register
            && !changed_previous
            && !has_rename_source
        {
            return None;
        }
        let occupancy_revision = previous
            .filter(|holder| {
                holder.worker_id == worker_id && holder.session_id == target_session_id
            })
            .map(|holder| holder.occupancy_revision)
            .unwrap_or_else(|| {
                state
                    .name_occupancy_revision(context.project_scope.as_str(), target_name)
                    .checked_add(1)
                    .unwrap_or(0)
            });
        Some(crate::server::binding_slot::PendingEffect {
            kind,
            role: None,
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
            pane_anchor,
            created_ms: crate::server::now_ms(),
        })
    }

    /// Resolve the stable name a restored identity already holds, without
    /// inventing one. `None` means the name owner has not decided this
    /// carrier's name yet; that decision happens once later from live facts,
    /// so a refresh must not assign a pool name and preempt it here.
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
        facts: Option<&crate::proto::IdentityFacts>,
        tmux_session_name: Option<&str>,
        pane_anchor: Option<crate::server::name_state::NamePaneAnchor>,
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
        prepared.typed.envelope.validate().map_err(|error| {
            format!(
                "typed registrar rejected registration: {}",
                notification_contract::MutationError::InvalidCommand(error.to_string())
            )
        })?;
        runtime
            .validate_typed_register(&state, &prepared.typed)
            .map_err(|error| format!("typed registrar rejected registration: {error}"))?;
        let control = self.binding_control_store().load()?;
        let registration = name_resolution::name_for_registration(&state, &control, binding)?;
        let name = registration.name.clone();
        let target_session_id = binding
            .session_id
            .as_ref()
            .ok_or("BINDING_CONTROL_REGISTER_REJECTED: candidate binding has no agent session")?
            .as_str()
            .to_owned();
        let effect_id = format!("register-{}", prepared.nested_operation_id);
        let effect = Self::handoff_effect(
            &control,
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
            pane_anchor.clone(),
        );
        let key = crate::server::binding_slot::IdentityBindingKey::new(
            context.project_scope.clone(),
            context.app_scope_id.clone(),
            name.clone(),
        );
        let mut value = crate::server::binding_slot::BindingValue::new(binding.clone(), transport);
        if let Some(facts) = facts {
            value.host.agent_kind = facts.agent_kind.clone();
            value.host.profile = facts.profile.clone();
        }
        if let Some(anchor) = pane_anchor.as_ref() {
            value.invocation_cwd = anchor.cwd.clone();
        }
        value.registration_number = registration.registration_number;
        Self::prepare_control_value(&control, &key, &mut value)?;
        let route = binding.route_scope();
        let is_current_master = current_master_worker_id(&state, Some(&route)).as_deref()
            == Some(binding.agent_id.as_str());
        let new_principal = !state.workers.contains_key(&worker.id)
            && !control
                .active_values()
                .any(|current| current.binding.same_principal(binding));
        drop(state);
        let staged = if new_principal {
            Some(
                crate::identity::stage_registration_credential(
                    &self.host.host_paths,
                    &Scope {
                        root: runtime.root.clone(),
                    },
                    crate::identity::Identity {
                        worker_id: worker.id.clone(),
                        token: worker.token.clone(),
                        project_scope: None,
                        runtime: None,
                        transport: None,
                    },
                    crate::identity::RuntimeIdentity {
                        agent_id: binding.agent_id.clone(),
                        runtime_id: binding.runtime_id.clone(),
                        appserver_id: binding.app_scope_id.clone(),
                        binding_id: binding.binding_id.clone(),
                        session_id: binding.session_id.clone(),
                        native_thread_id: binding.native_thread_id.clone(),
                    },
                    value.transport.clone(),
                )
                .map_err(|error| format!("{error:#}"))?,
            )
        } else {
            None
        };
        let rename = match crate::server::identity_context::rename_prepared_anchor(
            &value,
            &value.invocation_cwd,
            &name,
            is_current_master,
            false,
            None,
        ) {
            Ok(rename) => rename,
            Err(error) => {
                return Err(match staged.map(|staged| staged.rollback()).transpose() {
                    Ok(_) => error,
                    Err(rollback) => format!("{error}; CREDENTIAL_ROLLBACK_FAILED: {rollback:#}"),
                })
            }
        };
        let change = match effect {
            Some(effect) => crate::server::binding_store::ControlChange::CommitWithEffect {
                key,
                value,
                expected_generation: None,
                effect,
            },
            None => crate::server::binding_store::ControlChange::Commit {
                key,
                value,
                expected_generation: None,
            },
        };
        let committed = match self.commit_control_change_locked(change) {
            Ok(committed) => committed,
            Err(mut error) => {
                let rename = rename.map(|rename| rename.rollback()).transpose();
                let credential = staged.map(|staged| staged.rollback()).transpose();
                if let Err(rollback) = rename {
                    error = format!("{error}; TMUX_SESSION_RENAME_ROLLBACK_FAILED: {rollback}");
                }
                if let Err(rollback) = credential {
                    error = format!("{error}; CREDENTIAL_ROLLBACK_FAILED: {rollback:#}");
                }
                return Err(error);
            }
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
        let current = self.binding_control_store().load()?;
        if current.pending_effect(&effect.effect_id) != Some(effect) {
            return Ok(());
        }
        let control = &current;
        if let Some(primary) = effect.effect_id.strip_suffix(":outgoing") {
            if self
                .binding_control_store()
                .load()?
                .pending_effect(primary)
                .is_some()
            {
                return Err(format!("PENDING_EFFECT_PRIMARY_INCOMPLETE: {primary}"));
            }
        }
        if effect.kind == crate::server::binding_slot::PendingEffectKind::Retire {
            let key = crate::server::binding_slot::IdentityBindingKey::new(
                effect.project_scope.clone(),
                effect.app_scope_id.clone(),
                &effect.name,
            );
            let slot = control.slot(&key).ok_or("PENDING_EFFECT_MAPPING_MISSING")?;
            // A new current mapping supersedes this retired binding. Never
            // consume the prior intent against its successor.
            if slot.active_binding().is_some() {
                return self.clear_pending_effect_locked(&effect.effect_id);
            }
            let value = slot
                .reference_previous()
                .ok_or("PENDING_EFFECT_RETIREMENT_BINDING_MISSING")?;
            if value.binding.agent_id.as_str() != effect.target_worker_id
                || value.binding.session_id.as_ref().map(|id| id.as_str())
                    != Some(effect.target_session_id.as_str())
            {
                return self.clear_pending_effect_locked(&effect.effect_id);
            }
            let runtime = self
                .routes
                .lock()
                .unwrap()
                .get(&(
                    effect.app_scope_id.to_string(),
                    effect.project_scope.as_str().to_owned(),
                ))
                .and_then(|route| route.runtime.clone())
                .ok_or("PENDING_EFFECT_RUNTIME_MISSING")?;
            let token = runtime
                .state
                .lock()
                .unwrap()
                .workers
                .get(&effect.target_worker_id)
                .map(|worker| worker.token.clone());
            if let Some(token) = token {
                let context = ProjectContext {
                    project_scope: effect.project_scope.clone(),
                    app_scope_id: effect.app_scope_id.clone(),
                    canonical_root: runtime.root.to_string_lossy().into_owned(),
                    runtime_context: None,
                };
                let response = peer_lifecycle::unregister_self(
                    &runtime,
                    Some(&context),
                    &effect.target_worker_id,
                    &token,
                    Some(&value.binding),
                );
                if !response.ok {
                    return Err(response
                        .error
                        .unwrap_or_else(|| "PENDING_EFFECT_RETIREMENT_FAILED".into()));
                }
            }
            crate::identity::clear_persisted_identity(
                &self.host.host_paths,
                &effect.target_worker_id,
            )
            .map_err(|error| format!("PENDING_EFFECT_RETIREMENT_CREDENTIAL_FAILED: {error}"))?;
            if crate::server::global_state::installed_route_address_key(&value.binding).is_some() {
                self.host
                    .commit_checked(&[Event::GlobalCurrentThreadRouteRetired {
                        binding: value.binding.clone(),
                    }])
                    .map_err(|error| format!("PENDING_EFFECT_RETIREMENT_ROUTE_FAILED: {error}"))?;
            }
            return self.clear_pending_effect_locked(&effect.effect_id);
        }
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
        let slot_key = crate::server::binding_slot::IdentityBindingKey::new(
            effect.project_scope.clone(),
            effect.app_scope_id.clone(),
            effect.name.clone(),
        );
        let slot = control
            .slot(&slot_key)
            .ok_or("PENDING_EFFECT_MAPPING_MISSING")?;
        let value = slot
            .active_binding()
            .ok_or("PENDING_EFFECT_MAPPING_INACTIVE")?;
        if matches!(
            effect.kind,
            crate::server::binding_slot::PendingEffectKind::Name
                | crate::server::binding_slot::PendingEffectKind::Register
        ) {
            self.materialize_current_worker_locked(&runtime, value, Some(effect))?;
        }
        let mut state = runtime.state.lock().unwrap();
        if !state.workers.contains_key(&effect.target_worker_id) {
            return Err(format!(
                "PENDING_EFFECT_TARGET_MISSING: effect {} targets worker {} with no committed registration",
                effect.effect_id, effect.target_worker_id
            ));
        }
        let mut role_events = Vec::new();
        if let Some(role) = &effect.role {
            use crate::server::binding_slot::{RoleAction, RoleAuthorization};
            let current = current_master_grant(&state, Some(&route_scope));
            let applied = match role.action {
                RoleAction::SetMaster => current.as_ref().is_some_and(|grant| {
                    grant.agent_id.as_str() == effect.target_worker_id
                        && grant.binding_id == value.binding.binding_id
                }),
                RoleAction::ClearMaster => current.is_none(),
            };
            if !applied {
                let (granted_by, authorization) = match &role.authorization {
                    RoleAuthorization::Delegate {
                        actor,
                        target,
                        grant_id,
                        grant_generation,
                    } => {
                        let grant = current.ok_or(
                            "MASTER_DELEGATE_DENIED: caller no longer holds the accepted grant",
                        )?;
                        if actor != grant.agent_id.as_str()
                            || grant_id != grant.resource_id()
                            || *grant_generation != grant.resource_generation()
                            || target != &effect.target_worker_id
                        {
                            return Err(
                                "MASTER_DELEGATE_DENIED: accepted grant ownership changed".into()
                            );
                        }
                        (actor.as_str(), "delegated by the current master")
                    }
                    RoleAuthorization::User {
                        authorization,
                        granted_by,
                    } => {
                        if authorization.trim().is_empty()
                            || authorization.chars().any(char::is_control)
                            || granted_by.trim().is_empty()
                            || granted_by.chars().any(char::is_control)
                        {
                            return Err("ROLE_AUTHORIZATION_INVALID".into());
                        }
                        (granted_by.as_str(), authorization.as_str())
                    }
                };
                match role.action {
                    RoleAction::SetMaster => {
                        let grant = master_grant_for_worker(
                            &state,
                            &route_scope,
                            &effect.target_worker_id,
                            granted_by,
                            authorization,
                        )?;
                        role_events = master_authority_transfer_events(&state, &route_scope, grant);
                    }
                    RoleAction::ClearMaster => {
                        role_events = master_authority_revoke_events(&state, &route_scope)
                    }
                }
            }
        }
        if effect.kind == crate::server::binding_slot::PendingEffectKind::Name {
            let current = state
                .global
                .lookup_binding_for(&route_scope, &value.binding.binding_id)
                .cloned()
                .ok_or(
                    "PENDING_EFFECT_BINDING_MISSING: exact existing principal binding is absent",
                )?;
            if current != value.binding
                || state.workers[&effect.target_worker_id].transport.as_ref()
                    != Some(&value.transport)
            {
                if current != value.binding
                    && (!current.same_principal(&value.binding)
                        || slot
                            .reference_previous()
                            .is_none_or(|previous| previous.binding != current))
                {
                    return Err("PENDING_EFFECT_BINDING_CONFLICT: exact prior mapping differs from the registered binding".into());
                }
                let mut worker = state.workers[&effect.target_worker_id].clone();
                worker.transport = Some(value.transport.clone());
                let mut events = vec![
                    Event::GlobalRuntimeBound {
                        binding: value.binding.clone(),
                    },
                    Event::Registered { worker },
                ];
                if crate::server::global_state::installed_route_address_key(&current).is_some() {
                    events.insert(
                        0,
                        Event::GlobalCurrentThreadRouteRetired {
                            binding: current.clone(),
                        },
                    );
                }
                runtime
                    .commit_locked_checked(&mut state, &events)
                    .map_err(|error| format!("PENDING_EFFECT_BINDING_COMMIT_FAILED: {error}"))?;
            }
        }
        if effect.kind == crate::server::binding_slot::PendingEffectKind::Register
            && (state
                .global
                .lookup_binding_for(&route_scope, &value.binding.binding_id)
                != Some(&value.binding)
                || !state
                    .global
                    .command_receipts
                    .values()
                    .any(|receipt| receipt.operation_id.as_str() == effect.operation_id))
        {
            // Only a real Register receipt can materialize its business
            // effects. The map never mints a missing worker or credential.
            return Err(format!(
                "PENDING_EFFECT_REGISTER_INCOMPLETE: effect {} has no completed business Register",
                effect.effect_id
            ));
        }
        let mut events = Vec::new();
        if !Self::pending_effect_complete(&state, effect) || !role_events.is_empty() {
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
            events = crate::server::name_resolution::build_name_resolution_events_with_rename_from(
                &state,
                effect.project_scope.as_str(),
                &occupancy,
                rename_sources,
                state
                    .name_occupancy(effect.project_scope.as_str(), &effect.name)
                    .filter(|holder| {
                        Some(holder.worker_id.as_str()) == effect.previous_worker_id.as_deref()
                            && Some(holder.session_id.as_str())
                                == effect.previous_session_id.as_deref()
                    }),
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
            events.extend(role_events);
        }
        // Name, assignment, resource and ticket recovery are decided from one
        // candidate view. The candidate is memory-only; the real state is
        // committed once below.
        let mut candidate = state.clone();
        for event in &events {
            candidate
                .apply_checked(event)
                .map_err(|error| format!("PENDING_EFFECT_CANDIDATE_FAILED: {error}"))?;
        }
        // Occupancy can already be current when only the native thread changed.
        // Ticket recovery therefore has its own admission condition here.
        events.extend(
            ticket_binding_recovery_events(&candidate, control, &route_scope, &effect.name)
                .map_err(|error| {
                    format!(
                        "TICKET_BINDING_RECOVERY_FAILED: {}",
                        error
                            .error
                            .unwrap_or_else(|| "unknown recovery error".into())
                    )
                })?,
        );
        if !events.is_empty() {
            runtime
                .commit_locked_checked(&mut state, &events)
                .map_err(|error| format!("PENDING_EFFECT_COMMIT_FAILED: {error}"))?;
        }
        let mut identity = self
            .recovered_binding_identity(
                &ProjectContext {
                    app_scope_id: effect.app_scope_id.clone(),
                    canonical_root: runtime.root.to_string_lossy().into_owned(),
                    project_scope: effect.project_scope.clone(),
                    runtime_context: None,
                },
                &value.binding,
                &state,
            )
            .ok_or("PENDING_EFFECT_CREDENTIAL_MISSING")?;
        identity.transport = Some(value.transport.clone());
        drop(state);
        crate::identity::persist_current_binding_at(
            &self.host.host_paths,
            &Scope {
                root: runtime.root.clone(),
            },
            &mut identity,
            crate::identity::RuntimeIdentity {
                agent_id: value.binding.agent_id.clone(),
                runtime_id: value.binding.runtime_id.clone(),
                appserver_id: value.binding.app_scope_id.clone(),
                binding_id: value.binding.binding_id.clone(),
                session_id: value.binding.session_id.clone(),
                native_thread_id: value.binding.native_thread_id.clone(),
            },
            value.transport.clone(),
        )
        .map_err(|error| format!("PENDING_EFFECT_CREDENTIAL_FAILED: {error}"))?;
        if matches!(
            effect.kind,
            crate::server::binding_slot::PendingEffectKind::Name
                | crate::server::binding_slot::PendingEffectKind::Register
        ) {
            let mut publications = Vec::new();
            if let Some(previous) = slot.reference_previous().filter(|previous| {
                previous.binding != value.binding
                    && Some(previous.binding.agent_id.as_str())
                        == effect.previous_worker_id.as_deref()
                    && previous.binding.session_id.as_ref().map(|id| id.as_str())
                        == effect.previous_session_id.as_deref()
                    && crate::server::global_state::installed_route_address_key(&previous.binding)
                        .is_some()
            }) {
                publications.push(Event::GlobalCurrentThreadRouteRetired {
                    binding: previous.binding.clone(),
                });
            }
            if let Some(owner) = value.binding.tmux_endpoint.as_ref().and_then(|endpoint| {
                self.host
                    .state
                    .lock()
                    .unwrap()
                    .global
                    .lookup_unique_tmux_pane_route(endpoint)
                    .cloned()
            }) {
                if owner != value.binding {
                    publications.push(Event::GlobalCurrentThreadRouteRetired { binding: owner });
                }
            }
            if !self.binding_publication_matches(&value.binding)
                && crate::server::global_state::installed_route_address_key(&value.binding)
                    .is_some()
            {
                publications.push(Event::GlobalCurrentThreadRouteSet {
                    binding: value.binding.clone(),
                });
            }
            if !publications.is_empty() {
                self.host
                    .commit_checked(&publications)
                    .map_err(|error| format!("PENDING_EFFECT_ROUTE_FAILED: {error}"))?;
            }
        }
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
                        || value
                            .binding
                            .session_id
                            .as_ref()
                            .map(|session| session.as_str())
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
        let Some(holder) =
            state.name_occupancy(effect.project_scope.as_str(), effect.name.as_str())
        else {
            return false;
        };
        if holder.worker_id != effect.target_worker_id
            || holder.session_id != effect.target_session_id
            || holder.occupancy_revision < effect.occupancy_revision
        {
            return false;
        }
        if !effect.moves_existing_names {
            return true;
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
        facts: Option<crate::proto::IdentityFacts>,
        tmux_session_name: Option<String>,
        pane_anchor: Option<crate::server::name_state::NamePaneAnchor>,
        effect_id_out: &'a mut Option<String>,
    ) -> impl FnMut(&crate::server::PreparedRegisterEnvelope) -> Result<(), String> + 'a {
        move |prepared| {
            *effect_id_out = Some(self.publish_register_control_before_consume(
                runtime,
                prepared,
                facts.as_ref(),
                tmux_session_name.as_deref(),
                pane_anchor.clone(),
            )?);
            Ok(())
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
        let mut bindings = Vec::new();
        for value in controlled.active_values() {
            let has_route_address =
                crate::server::global_state::installed_route_address_key(&value.binding).is_some();
            let complete_rex = value
                .transport
                .rex_endpoint
                .as_ref()
                .is_some_and(|endpoint| {
                    !endpoint.server.trim().is_empty()
                        && !endpoint.session_id.trim().is_empty()
                        && !endpoint.block_id.trim().is_empty()
                });
            if !has_route_address && !complete_rex {
                append_log(
                    &self.host.log_path(),
                    &format!(
                        "BINDING_CONTROL_ROUTE_PENDING: binding {} retains its identity; current delivery facts are missing",
                        value.binding.binding_id
                    ),
                );
                continue;
            }
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
            bindings.push(value.binding.clone());
        }
        for binding in &bindings {
            let unresolved_pane = binding.tmux_endpoint.as_ref().is_some_and(|endpoint| {
                let conflicting_control = bindings.iter().any(|other| {
                    let same_claim =
                        other.binding_id == binding.binding_id && other.same_principal(binding);
                    !same_claim
                        && other.tmux_endpoint.as_ref().is_some_and(|other_endpoint| {
                            crate::client::adapters::tmux::same_owned_pane(endpoint, other_endpoint)
                        })
                });
                conflicting_control
                    && self
                        .host
                        .state
                        .lock()
                        .unwrap()
                        .global
                        .lookup_unique_tmux_pane_route(endpoint)
                        .is_none()
            });
            if unresolved_pane {
                // No current host owner resolves these conflicting historical
                // values. Keep them unpublished until a real context request
                // supplies the current facts and authority; do not pick one.
                continue;
            }
            match self.reconcile_binding_publication(binding) {
                Ok(()) | Err(BindingPublicationError::AddressConflict(_)) => {}
                Err(BindingPublicationError::Persistence(error)) => return Err(error),
            }
        }
        Ok(())
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
