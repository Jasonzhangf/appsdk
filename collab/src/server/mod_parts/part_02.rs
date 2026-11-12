#[cfg(test)]
#[path = "../lifecycle_fence_tests.rs"]
mod lifecycle_fence_tests;

/// A Register command prepared from one consistent reducer snapshot.
///
/// The nested `command_id`/`operation_id` in `typed` are the durable outer
/// identifiers bound before consume. `consume_prepared_register` must send
/// this exact envelope; it never regenerates the identifiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PreparedRegisterEnvelope {
    pub(crate) nested_command_id: String,
    pub(crate) nested_operation_id: String,
    pub(crate) typed: TypedEnvelope,
}

/// Current invocation's Register inputs and exact command IDs.
#[derive(Clone)]
pub(crate) struct RegisterOuterBinding {
    explicit_peer: bool,
    /// This context invocation's daemon-observed fixed tmux session name. It
    /// travels with the context-driven Register so the first name decision uses
    /// the same anchor the context owner will project.
    name_tmux_session_name: Option<String>,
    /// The daemon-observed pane and invocation cwd from this context request.
    /// The first Register name decision uses the same joint fallback anchor the
    /// later context name resolution will use.
    name_pane_anchor: Option<crate::server::name_state::NamePaneAnchor>,
    identity_facts: Option<crate::proto::IdentityFacts>,
    rex_recovery: Option<crate::server::identity_context::RexRecoveryObservation>,
}

impl RegisterOuterBinding {
    pub(crate) fn new(
    ) -> Self {
        Self {
            explicit_peer: false,
            name_tmux_session_name: None,
            name_pane_anchor: None,
            identity_facts: None,
            rex_recovery: None,
        }
    }

    pub(crate) fn with_identity_facts(mut self, facts: crate::proto::IdentityFacts) -> Self {
        self.identity_facts = Some(facts);
        self
    }

    pub(crate) fn identity_facts(&self) -> Option<&crate::proto::IdentityFacts> {
        self.identity_facts.as_ref()
    }

    pub(crate) fn with_rex_recovery(
        mut self,
        recovery: Option<crate::server::identity_context::RexRecoveryObservation>,
    ) -> Self {
        self.rex_recovery = recovery;
        self
    }

    pub(crate) fn rex_recovery(
        &self,
    ) -> Option<&crate::server::identity_context::RexRecoveryObservation> {
        self.rex_recovery.as_ref()
    }

    pub(crate) fn with_name_tmux_session_name(
        mut self,
        tmux_session_name: Option<String>,
    ) -> Self {
        self.name_tmux_session_name = tmux_session_name;
        self
    }

    pub(crate) fn with_name_pane_anchor(
        mut self,
        pane_anchor: Option<crate::server::name_state::NamePaneAnchor>,
    ) -> Self {
        self.name_pane_anchor = pane_anchor;
        self
    }

    /// The daemon-observed fixed tmux session name this context invocation
    /// carries, if any. Only the nested Register name decision consumes it.
    pub(crate) fn name_tmux_session_name(&self) -> Option<&str> {
        self.name_tmux_session_name.as_deref()
    }

    /// The joint daemon-observed pane+cwd anchor this context invocation
    /// carries, if any. Only the nested Register name decision consumes it.
    pub(crate) fn name_pane_anchor(&self) -> Option<&crate::server::name_state::NamePaneAnchor> {
        self.name_pane_anchor.as_ref()
    }

    pub(crate) fn with_role_intent(mut self, request: &IdentityContextRequest) -> Self {
        self.explicit_peer = request.grant_approval.as_ref().or(request.approval.as_ref())
            .is_some_and(|approval| approval["action"] == "restore_peer");
        self
    }


}

/// The sole material algorithm for newly prepared Register identifiers.
fn scoped_register_command_prefix(
    scope: &RouteScope,
    binding: &BindingId,
) -> String {
    use sha2::{Digest, Sha256};
    let material = serde_json::to_vec(&(
        "collab-register-scope/v1",
        &scope.project_scope_id,
        &scope.app_scope_id,
        binding,
    ))
    .expect("typed Register ID tuple contains only scalar values");
    format!("register-{:x}", Sha256::digest(material))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RegisterOrigin {
    Registration,
    CurrentPrincipal,
    InitialCurrentContinuation { explicit_peer: bool },
}

impl Server {
    pub(crate) fn log_path_for(root: &Path) -> PathBuf {
        root.join(".agent-collab").join("server").join("log.txt")
    }

    pub fn log_path(&self) -> PathBuf {
        Self::log_path_for(&self.storage_root)
    }

    #[cfg(test)]
    pub(crate) fn typed_register_envelope(
        &self,
        worker_id: &str,
        token: &str,
        thread_id: &str,
        worker_cwd: &str,
    ) -> Result<TypedEnvelope, String> {
        let transport = SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/collab-test-appserver.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some(format!("session-{worker_id}")),
            thread_id: Some(thread_id.to_string()),
            tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: vec!["send_message_to_thread".into()],
            self_check: "test appserver transport".into(),
        };
        let project_scope = GlobalState::canonical_project_scope(std::path::Path::new(worker_cwd))
            .map_err(|error| error.to_string())?;
        self.typed_register_envelope_for_scope(
            worker_id,
            token,
            &transport,
            project_scope,
            worker_cwd,
            AppServerId::new("tui-default").map_err(|error| error.to_string())?,
            false,
        )
    }

    fn typed_register_envelope_for_scope(
        &self,
        worker_id: &str,
        token: &str,
        transport: &SelectedTransport,
        project_scope: ProjectScopeId,
        worker_cwd: &str,
        app_scope: AppServerId,
        reuse_existing: bool,
    ) -> Result<TypedEnvelope, String> {
        Ok(self
            .prepare_register_envelope_for_scope(
                worker_id,
                token,
                transport,
                project_scope,
                worker_cwd,
                app_scope,
                reuse_existing,
            )?
            .typed)
    }

    fn prepare_register_envelope_for_scope(
        &self,
        worker_id: &str,
        token: &str,
        transport: &SelectedTransport,
        project_scope: ProjectScopeId,
        worker_cwd: &str,
        app_scope: AppServerId,
        reuse_existing: bool,
    ) -> Result<PreparedRegisterEnvelope, String> {
        self.prepare_register_envelope_for_current(
            worker_id, token, transport, project_scope, worker_cwd, app_scope,
            reuse_existing, None, None,
        )
    }

    fn prepare_register_envelope_for_current(
        &self,
        worker_id: &str,
        token: &str,
        transport: &SelectedTransport,
        project_scope: ProjectScopeId,
        worker_cwd: &str,
        app_scope: AppServerId,
        reuse_existing: bool,
        current: Option<&RuntimeBinding>,
        pending_operation: Option<&str>,
    ) -> Result<PreparedRegisterEnvelope, String> {
        let binding_text = sanitize_identifier(&format!("binding-{worker_id}"));
        let binding_id = current.map(|binding| binding.binding_id.clone())
            .map(Ok).unwrap_or_else(|| BindingId::new(binding_text.clone()))
            .map_err(|error| error.to_string())?;
        let route_scope = RouteScope {
            app_scope_id: app_scope.clone(),
            project_scope_id: project_scope.clone(),
        };
        // Derive IDs and the binding from one snapshot. Registration is
        // bound to its principal, not to unrelated
        // ledger or timer revisions. Explicit caller CAS remains supported.
        let (
            runtime_text,
            registration,
            registered_ms,
            snapshot_revision,
            base_command,
            prior_attempt,
            pending_attempt,
        ) = {
            let st = self.state.lock().unwrap();
            let runtime_text = if let Some(current) = current {
                current.runtime_id.as_str().to_owned()
            } else {
                match st.global.lookup_binding_for(&route_scope, &binding_id) {
                    Some(existing) if existing.agent_id.as_str() == worker_id => {
                        let runtime_text = existing.runtime_id.as_str().to_owned();
                        runtime_text
                    }
                    Some(_) => {
                        return Err(format!(
                        "RUNTIME_BINDING_REJECTED: binding {binding_id} belongs to another worker"
                    ));
                    }
                    None => {
                        let runtime_text =
                            match (
                                transport.session_id.as_deref(),
                                transport.thread_id.as_deref(),
                            ) {
                                _ if transport.kind == TransportKind::Tmux => {
                                    format!("runtime-tmux-{}", sanitize_identifier(worker_id))
                                }
                                _ if transport.kind == TransportKind::Rex => {
                                    format!("runtime-rex-{}", sanitize_identifier(worker_id))
                                }
                                (Some(session_id), Some(thread_id)) => format!(
                                    "runtime-{}-{}-{}",
                                    transport.kind.as_str(),
                                    sanitize_identifier(session_id),
                                    sanitize_identifier(thread_id)
                                ),
                                // The fallback names the kinds that arrive without a
                                // verified session/thread pair. App Server keeps its
                                // historical label because existing registrations
                                // already route on it; no other kind may claim an App
                                // Server transport it does not have.
                                _ => match &transport.kind {
                                    TransportKind::AppServer => {
                                        format!("runtime-{}-appserver", transport.kind.as_str())
                                    }
                                    other => format!("runtime-{}", other.as_str()),
                                },
                            };
                        runtime_text
                    }
                }
            };
            let registration = st
                .global
                .lookup_registration(&project_scope, &app_scope)
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| {
                    global_state::ProjectRegistration::with_registered_at(
                        project_scope.clone(),
                        app_scope.clone(),
                        now_ms(),
                    )
                })
                .map_err(|error| error.to_string())?;
            let registered_ms = st
                .workers
                .get(worker_id)
                .map(|worker| worker.registered_ms)
                .unwrap_or_else(now_ms);
            let base_command =
                scoped_register_command_prefix(&route_scope, &binding_id);
            // `prior_attempt` selects the retry suffix, so it must be read in
            // the same snapshot as `generation` and the retry revision.
            let prior_attempt = !reuse_existing
                && st
                    .global
                    .lookup_command_receipt(
                        &CommandId::new(base_command.clone()).map_err(|error| error.to_string())?,
                    )
                    .is_some();
            (
                runtime_text,
                registration,
                registered_ms,
                st.revision,
                base_command,
                prior_attempt,
                pending_operation.filter(|operation| !st.global.command_receipts.values()
                    .any(|receipt| receipt.operation_id.as_str() == *operation)).map(str::to_owned),
            )
        };
        let agent_id = AgentId::new(worker_id.to_string()).map_err(|error| error.to_string())?;
        let runtime_id = RuntimeId::new(runtime_text).map_err(|error| error.to_string())?;
        let native_thread_id = transport
            .thread_id
            .as_ref()
            .map(|value| NativeThreadId::new(value.clone()))
            .transpose()
            .map_err(|error| error.to_string())?;
        let session_id = transport
            .session_id
            .as_ref()
            .map(|value| crate::identity::SessionId::new(value.clone()))
            .transpose()
            .map_err(|error| error.to_string())?;
        if transport.kind != TransportKind::Tmux && session_id.is_none() {
            return Err("App Server registration requires session_id".into());
        }
        let mut binding = RuntimeBinding::new_with_session(
            project_scope.clone(),
            app_scope,
            agent_id,
            runtime_id,
            binding_id.clone(),
            session_id,
            native_thread_id,
        )
        .map_err(|error| error.to_string())?;
        binding.tmux_endpoint = transport.tmux_endpoint.clone();
        if let Some(current) = current {
            binding = current.clone();
        }
        binding.validate().map_err(|error| error.to_string())?;
        let attempt_suffix = prior_attempt
            .then(|| format!("-retry-{snapshot_revision}"))
            .unwrap_or_default();
        let attempt_suffix = if let Some(operation) = pending_attempt.as_deref() {
            let prefix = format!("register-op-{}", base_command.strip_prefix("register-").unwrap());
            let suffix = operation.strip_prefix(&prefix)
                .ok_or("PENDING_EFFECT_REGISTER_OPERATION_MISMATCH")?;
            if !suffix.is_empty() && !suffix.strip_prefix("-retry-")
                .is_some_and(|revision| !revision.is_empty() && revision.bytes().all(|byte| byte.is_ascii_digit()))
            {
                return Err("PENDING_EFFECT_REGISTER_OPERATION_MISMATCH".into());
            }
            suffix.to_owned()
        } else {
            attempt_suffix
        };
        let command_id = CommandId::new(format!("{base_command}{attempt_suffix}"))
            .map_err(|error| error.to_string())?;
        let operation_id = OperationId::new(format!(
            "register-op-{}{attempt_suffix}",
            base_command.strip_prefix("register-").unwrap()
        ))
        .map_err(|error| error.to_string())?;
        let nested_command_id = command_id.as_str().to_owned();
        let nested_operation_id = operation_id.as_str().to_owned();
        let envelope = CommandEnvelope::new(
            command_id,
            operation_id,
            binding_id,
            route_scope,
            None,
            None,
            None,
            None,
        );
        let worker = WorkerRec {
            id: worker_id.to_string(),
            token: token.to_string(),
            cwd: worker_cwd.to_string(),
            registered_ms,
            transport: Some(transport.clone()),
        };
        Ok(PreparedRegisterEnvelope {
            nested_command_id,
            nested_operation_id,
            typed: TypedEnvelope {
                command: TypedCommand::RegisterWorker {
                    registration,
                    binding,
                    worker,
                },
                envelope,
            },
        })
    }

    /// Plan the exact execution releases needed by one incoming Register.
    /// The caller owns the register gate and current-map snapshot. Candidates
    /// stay inside the incoming project/app scope, and any current relation
    /// that still protects the old principal blocks the release.
    fn register_execution_release_events(
        &self,
        state: &State,
        control: &binding_slot::BindingControlSet,
        incoming: &RuntimeBinding,
    ) -> Result<Vec<Event>, String> {
        let Some(project) = state.global.lookup_project(&incoming.project_scope) else {
            return Ok(Vec::new());
        };
        let candidates: Vec<RuntimeBinding> = project
            .runtime_bindings
            .values()
            .filter(|old| {
                old.project_scope == incoming.project_scope
                    && old.app_scope_id == incoming.app_scope_id
                    && old.runtime_id == incoming.runtime_id
                    && old.binding_id != incoming.binding_id
            })
            .cloned()
            .collect();

        let mut events = Vec::new();
        for old in candidates {
            if control.active_values().any(|current| {
                &current.binding != incoming
                    && current.binding.same_principal(&old)
            }) {
                return Err(
                    "REGISTER_RUNTIME_RELEASE_REJECTED: current map still protects the prior principal"
                        .into(),
                );
            }
            if control.active_values().any(|current| {
                &current.binding != incoming
                    && current.binding.project_scope == incoming.project_scope
                    && current.binding.runtime_id == incoming.runtime_id
            }) {
                return Err(
                    "REGISTER_RUNTIME_RELEASE_REJECTED: another current still owns the runtime"
                        .into(),
                );
            }
            if state
                .global
                .lookup_master_grant_for(&old.route_scope(), &old.binding_id)
                .is_some()
            {
                events.push(Event::GlobalMasterRevoked {
                    project_scope: old.project_scope.clone(),
                    binding_id: old.binding_id.clone(),
                });
            }
            if global_state::installed_route_address_key(&old).is_some() {
                events.push(Event::GlobalCurrentThreadRouteRetired {
                    binding: old.clone(),
                });
            }
            events.push(Event::GlobalRuntimeExecutionReleased { binding: old });
        }
        Ok(events)
    }

    fn consume_register(
        &self,
        prepared: PreparedRegisterEnvelope,
        control: Option<&binding_slot::BindingControlSet>,
    ) -> Result<state::TypedOutcome, notification_contract::MutationError> {
        self.typed_dispatch_register_with_origin(
            prepared.typed,
            None,
            RegisterOrigin::Registration,
            control,
        )
    }

    /// Dispatch one typed command through validation and current-state commit.
    /// This is the production typed seam above the legacy
    /// CLI adapters; the legacy v1 call site routes through it.
    pub fn typed_dispatch(
        &self,
        typed: TypedEnvelope,
    ) -> Result<state::TypedOutcome, notification_contract::MutationError> {
        self.typed_dispatch_register(typed, None)
    }

    fn typed_dispatch_register(
        &self,
        typed: TypedEnvelope,
        outer_binding: Option<&RegisterOuterBinding>,
    ) -> Result<state::TypedOutcome, notification_contract::MutationError> {
        self.typed_dispatch_register_with_origin(
            typed,
            outer_binding,
            RegisterOrigin::Registration,
            None,
        )
    }

    fn typed_dispatch_register_with_origin(
        &self,
        typed: TypedEnvelope,
        outer_binding: Option<&RegisterOuterBinding>,
        origin: RegisterOrigin,
        control: Option<&binding_slot::BindingControlSet>,
    ) -> Result<state::TypedOutcome, notification_contract::MutationError> {
        typed.envelope.validate().map_err(|error| {
            notification_contract::MutationError::InvalidCommand(error.to_string())
        })?;
        let mut st = self.state.lock().unwrap();
        let TypedCommand::RegisterWorker { binding: incoming, .. } = &typed.command;
        let release_events = match control {
            Some(control) => self
                .register_execution_release_events(&st, control, incoming)
                .map_err(notification_contract::MutationError::InvalidCommand)?,
            None => Vec::new(),
        };
        let mut candidate = st.clone();
        let had_runtime_history = candidate
            .global
            .lookup_project(&incoming.project_scope)
            .is_some_and(|project| {
                project.runtime_bindings.values().any(|old| {
                    old.app_scope_id == incoming.app_scope_id
                })
            });
        for event in &release_events {
            candidate
                .apply_checked(event)
                .map_err(notification_contract::MutationError::InvalidState)?;
        }
        self.validate_typed_register(&candidate, &typed)?;
        let mut events = release_events;
        let TypedCommand::RegisterWorker { worker, binding, .. } = &typed.command;
        let first_business = match origin {
            RegisterOrigin::Registration => true,
            RegisterOrigin::CurrentPrincipal => false,
            RegisterOrigin::InitialCurrentContinuation { .. } => {
                !candidate.global.command_receipts.values().any(|receipt|
                    receipt.operation_id == typed.envelope.operation_id)
                    && !candidate.global.lookup_project(&binding.project_scope).is_some_and(|project|
                        project.runtime_bindings.values().any(|current| current.same_principal(binding)))
            }
        };
        for global_event in typed.command.global_events() {
            match global_event {
                GlobalEvent::ProjectRegistered { registration } => {
                    events.push(Event::GlobalProjectRegistered { registration })
                }
                GlobalEvent::RuntimeBound { binding } => {
                    // A pane has one owner, and an agent session has one
                    // current execution carrier. The later registrant
                    // replaces both claimants in this same commit, so the
                    // ledger never holds two current owners and cannot fence
                    // the new owner on the next attempt. A same-binding retry
                    // is excluded by the claimant filters below.
                    let mut superseded = std::collections::BTreeMap::<
                        (String, String),
                        (RuntimeBinding, &'static str),
                    >::new();
                    if let Some(endpoint) = binding.tmux_endpoint.as_ref()
                        .filter(|_| origin == RegisterOrigin::Registration)
                    {
                        let control = binding_store::BindingControlStore::new(self.host_paths.state_root())
                            .load().map_err(notification_contract::MutationError::InvalidCommand)?;
                        for claimant in pane_claimants(
                            &candidate,
                            binding.agent_id.as_str(),
                            &binding.route_scope(),
                            endpoint,
                        ) {
                            if control.holds_current_binding(&claimant) {
                                events.push(Event::GlobalCurrentThreadRouteRetired { binding: claimant });
                                continue;
                            }
                            superseded
                                .entry((
                                    claimant.project_scope.as_str().to_owned(),
                                    claimant.binding_id.as_str().to_owned(),
                                ))
                                .or_insert((
                                    claimant,
                                    "tmux pane reclaimed by a later registration",
                                ));
                        }
                    }
                    for claimant in session_claimants(
                        &candidate,
                        binding.agent_id.as_str(),
                        &binding.binding_id,
                        &binding.route_scope(),
                        &binding,
                    ).into_iter().filter(|_| origin == RegisterOrigin::Registration) {
                        superseded
                            .entry((
                                claimant.project_scope.as_str().to_owned(),
                                claimant.binding_id.as_str().to_owned(),
                            ))
                            .or_insert((
                                claimant,
                                "runtime session carrier replaced by a later registration",
                            ));
                    }
                    for (_, (claimant, reason)) in superseded {
                        events.extend(
                            retire_claimant_events(
                                binding.agent_id.as_str(),
                                &claimant,
                                reason,
                            )
                            .map_err(notification_contract::MutationError::InvalidCommand)?,
                        );
                    }
                    events.push(Event::GlobalRuntimeBound {
                        binding: binding.clone(),
                    });
                    if let Some(project) = candidate.global.lookup_project(&binding.project_scope)
                        .filter(|_| first_business)
                    {
                        if let Some(mut policy) = project
                            .bootstrap_policies
                            .get(binding.app_scope_id.as_str())
                            .filter(|policy| policy.consumption.is_none())
                            .cloned()
                        {
                            let explicit_peer = match origin {
                                RegisterOrigin::InitialCurrentContinuation { explicit_peer } => explicit_peer,
                                _ => outer_binding.is_some_and(|outer| outer.explicit_peer),
                            };
                            let history = had_runtime_history;
                            let incumbent = candidate.master_worker_id.is_some()
                                || project
                                    .master_grants
                                    .values()
                                    .any(|grant| grant.app_scope_id == binding.app_scope_id);
                            let outcome = if explicit_peer {
                                "explicit_peer"
                            } else if incumbent {
                                "incumbent"
                            } else if history {
                                "registration_history"
                            } else {
                                "master_granted"
                            };
                            if outcome == "master_granted" {
                                let grant = global_state::MasterGrant::new(
                                    binding.project_scope.clone(),
                                    binding.app_scope_id.clone(),
                                    binding.agent_id.clone(),
                                    "project",
                                    "user",
                                    &policy.authorization,
                                    binding.binding_id.clone(),
                                    now_ms(),
                                )
                                .map_err(|error| {
                                    notification_contract::MutationError::InvalidCommand(
                                        error.to_string(),
                                    )
                                })?;
                                events.push(Event::GlobalMasterGranted { grant });
                            }
                            policy.consumption = Some(global_state::BootstrapConsumption {
                                command_id: typed.envelope.command_id.as_str().to_owned(),
                                binding_id: binding.binding_id.clone(),
                                outcome: outcome.into(),
                            });
                            events.push(Event::GlobalBootstrapPolicySet { policy });
                        }
                    }
                }
                GlobalEvent::MasterGranted { grant } => {
                    events.push(Event::GlobalMasterGranted { grant })
                }
                GlobalEvent::MasterRevoked {
                    project_scope,
                    binding_id,
                } => events.push(Event::GlobalMasterRevoked {
                    project_scope,
                    binding_id,
                }),
                GlobalEvent::RuntimeBindingLedgerClassified { record } => {
                    events.push(Event::GlobalRuntimeBindingLedgerClassified { record })
                }
                GlobalEvent::LedgerScanReceiptRecorded { receipt } => {
                    events.push(Event::GlobalLedgerScanReceiptRecorded { receipt })
                }
            }
        }
        events.push(Event::Registered {
            worker: worker.clone(),
        });
        if let Some(transport) = selected_transport_for_worker(worker) {
            events.extend(default_direct_message_events(
                &candidate,
                &worker.id,
                &transport,
                now_ms(),
            ));
        }
        let mut notices = Vec::new();
        if first_business && !candidate.workers.contains_key(&worker.id) {
            let control = binding_store::BindingControlStore::new(self.host_paths.state_root()).load()
                .map_err(notification_contract::MutationError::InvalidCommand)?;
            let route = binding.route_scope();
            let current = control.current_for_worker(
                &route.project_scope_id, &route.app_scope_id, &worker.id,
            ).map_err(notification_contract::MutationError::InvalidCommand)?;
            if let Some(current) = current.filter(|slot|
                slot.identity_key.name != "Jenny"
                    && slot.active_binding().is_some_and(|current| current.binding == *binding))
            {
                if let Some(master) = current_master_worker_id(&candidate, Some(&route))
                    .filter(|master| master != &worker.id && candidate.workers.contains_key(master))
                {
                    if control.current_for_worker(&route.project_scope_id, &route.app_scope_id, &master)
                        .map_err(notification_contract::MutationError::InvalidCommand)?.is_some()
                    {
                        notices = relationship_notice_events(&candidate, &control, &route, &master,
                            "peer-registered", format!("{} 已注册为当前peer。读取 collab context / collab board show 查看当前成员及可分配工作；此通知不自动派单。", current.identity_key.name))
                            .map_err(notification_contract::MutationError::InvalidCommand)?;
                        events.extend(notices.clone());
                    }
                }
            }
        }
        let outcome = serde_json::json!({
            "command_id": typed.envelope.command_id.as_str(),
            "operation_id": typed.envelope.operation_id.as_str(),
            "scope": typed.envelope.scope,
        });
        let committed = self.commit_command_locked(
            &mut st,
            typed.envelope.command_id.as_str(),
            typed.envelope.operation_id.as_str(),
            &events,
            outcome,
            typed.envelope.expected_revision,
        )?;
        drop(st);
        if !committed.replayed {
            deliver_relationship_notices(self, &notices);
        }
        Ok(state::TypedOutcome {
            receipt: global_state::CommandReceipt {
                command_id: typed.envelope.command_id.clone(),
                operation_id: typed.envelope.operation_id.clone(),
                epoch: global_state::INITIAL_EPOCH,
                sequence: committed.receipt.sequence,
                revision: committed.receipt.revision,
                outcome: committed.outcome,
            },
            replayed: committed.replayed,
        })
    }

    fn validate_typed_register(
        &self,
        st: &State,
        typed: &TypedEnvelope,
    ) -> Result<(), notification_contract::MutationError> {
        let TypedCommand::RegisterWorker {
            registration,
            binding,
            worker,
        } = &typed.command;
        if binding.agent_id.as_str() != worker.id {
            return Err(notification_contract::MutationError::InvalidCommand(
                "runtime binding agent does not match worker identity".into(),
            ));
        }
        if registration.route_scope() != binding.route_scope() {
            return Err(notification_contract::MutationError::InvalidCommand(
                "project registration scope does not match runtime binding scope".into(),
            ));
        }
        if let Some(existing) = st.workers.get(&worker.id) {
            if existing.token != worker.token {
                let existing_route_scope =
                    existing_route_scope(st, &worker.id).map_err(|response| {
                        notification_contract::MutationError::InvalidCommand(
                            response
                                .error
                                .unwrap_or_else(|| "registration route is unavailable".into()),
                        )
                    })?;
                let same_route = existing_route_scope.as_ref().is_some_and(|route| {
                    route.project_scope_id == binding.project_scope
                        && route.app_scope_id == binding.app_scope_id
                });
                let existing_binding = existing_route_scope
                    .as_ref()
                    .and_then(|route| st.global.lookup_binding_for(route, &binding.binding_id));
                let same_runtime_thread = existing_binding.is_some_and(|current| {
                    current.agent_id == binding.agent_id
                        && current.native_thread_id == binding.native_thread_id
                        && current.session_id == binding.session_id
                });
                if !same_route || !same_runtime_thread {
                    return Err(notification_contract::MutationError::InvalidCommand(
                        "worker token does not belong to the registered runtime identity".into(),
                    ));
                }
            }
        }
        if typed.envelope.actor_binding_id != binding.binding_id {
            return Err(notification_contract::MutationError::InvalidCommand(
                format!(
                    "actor binding {} does not match command binding {}",
                    typed.envelope.actor_binding_id, binding.binding_id
                ),
            ));
        }
        if typed.envelope.scope != binding.route_scope() {
            return Err(notification_contract::MutationError::InvalidCommand(
                "envelope scope does not match binding route scope".into(),
            ));
        }
        if let Some(project) = st.global.lookup_project(&registration.project_scope) {
            let bound_apps = project
                .runtime_bindings
                .values()
                .map(|current| current.app_scope_id.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            let ambiguous_existing_owner =
                bound_apps.len() > 1 || (bound_apps.is_empty() && project.registrations.len() > 1);
            let different_existing_owner =
                bound_apps.len() == 1 && !bound_apps.contains(binding.app_scope_id.as_str());
            if ambiguous_existing_owner || different_existing_owner {
                return Err(notification_contract::MutationError::InvalidCommand(format!(
                    "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: resident project {} already has a different or ambiguous runtime app owner",
                    registration.project_scope.as_str()
                )));
            }
        }
        let mut next = st.global.clone();
        for event in typed.command.global_events() {
            event.apply(&mut next).map_err(|error| {
                notification_contract::MutationError::InvalidCommand(error.to_string())
            })?;
        }
        next.validate_binding(binding).map_err(|error| {
            notification_contract::MutationError::InvalidCommand(error.to_string())
        })?;
        Ok(())
    }
}
include!("part_02/commit.rs");

#[cfg(test)]
mod scoped_register_tests {
    use super::*;
    use crate::identity::SessionId;

    fn prepared_tmux_registration() -> (Arc<ProjectRuntimeManager>, Arc<Server>, PathBuf,
        peer_tests::IsolatedTmux, PreparedRegisterEnvelope) {
        let (server, root) = peer_tests::test_server();
        let server = Arc::new(server);
        let tmux = peer_tests::IsolatedTmux::start_single(&root);
        let mut endpoint = tmux.endpoints().remove(0);
        endpoint.codex_session_id = Some("rename-owner-session".into());
        endpoint.codex_thread_id = Some("rename-owner-thread".into());
        let transport = SelectedTransport {
            kind: TransportKind::Tmux, endpoint: Some(endpoint.socket_path.clone()),
            namespace: Some(endpoint.tmux_session_id.clone()),
            session_id: endpoint.codex_session_id.clone(), thread_id: endpoint.codex_thread_id.clone(),
            tmux_endpoint: Some(endpoint), capabilities: vec!["send_message_to_pane".into()],
            rex_endpoint: None,
            self_check: "test-owned existing pane".into(),
        };
        let prepared = server.prepare_register_envelope_for_scope(
            "rename-owner", "rename-owner-token", &transport,
            GlobalState::canonical_project_scope(&root).unwrap(), root.to_str().unwrap(),
            AppServerId::new("tui-default").unwrap(), false,
        ).unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
        (manager, server, root, tmux, prepared)
    }

    #[test]
    fn raw_registration_waits_for_the_existing_identity_gate() {
        let (manager, server, root, tmux, prepared) = prepared_tmux_registration();
        let TypedCommand::RegisterWorker { worker, binding, .. } = &prepared.typed.command;
        let context = ProjectContext::for_registered_root_with_app(&root,binding.app_scope_id.clone()).unwrap();
        let candidates = TransportCandidates {
            tmux:Some(crate::proto::TmuxCandidate {
                endpoint:worker.transport.as_ref().unwrap().tmux_endpoint.clone().unwrap(),
                cwd:root.to_string_lossy().into_owned(),
            }),..Default::default()
        };
        let request = Req::register(worker.id.clone(),worker.token.clone(),root.to_string_lossy().into_owned(),Some(candidates));
        let manager = Arc::new(manager);
        let guard = manager.identity_gate.lock().unwrap();
        let (started_tx,started_rx) = std::sync::mpsc::channel();
        let (tx,rx) = std::sync::mpsc::channel();
        let target = manager.clone();
        let child = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            tx.send(target.dispatch_sync(Some(context),request).1).unwrap();
        });
        started_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let blocked = matches!(rx.recv_timeout(std::time::Duration::from_secs(1)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout));
        drop(guard);
        let response = if blocked { Some(rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap()) } else { None };
        child.join().unwrap();
        assert!(blocked,"raw Register bypassed the current identity transaction gate");
        assert!(response.unwrap().ok);
        drop(manager);
        drop(server);
        drop(tmux);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn register_control_publish_error_restores_the_actual_physical_name() {
        use std::os::unix::fs::PermissionsExt;
        let (manager, server, root, tmux, prepared) = prepared_tmux_registration();
        let endpoint = tmux.endpoints().remove(0);
        let previous = crate::client::adapters::tmux::require_live_session_name(&endpoint).unwrap();
        let parent = server.host_paths.state_root();
        std::fs::create_dir_all(parent).unwrap();
        let credential = parent.join("identities/rename-owner/identity.json");
        std::fs::create_dir_all(credential.parent().unwrap()).unwrap();
        let permissions = std::fs::metadata(parent).unwrap().permissions();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = manager.publish_register_control_before_consume(&server, &prepared, None, None, None);
        std::fs::set_permissions(parent, permissions).unwrap();
        let error = result.unwrap_err();
        assert!(error.starts_with("BINDING_CONTROL_SAVE_FAILED: create temp"), "{error}");
        assert_eq!(crate::client::adapters::tmux::require_live_session_name(&endpoint).unwrap(), previous);
        assert!(!parent.join("identity-map.json").exists());
        assert!(!credential.exists(), "unpublished Register retained its staged credential");
        assert!(server.state.lock().unwrap().workers.is_empty());
        drop(tmux);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn register_credential_failure_keeps_the_original_name_and_unpublished_map() {
        use std::os::unix::fs::PermissionsExt;
        let (manager, server, root, tmux, prepared) = prepared_tmux_registration();
        let endpoint = tmux.endpoints().remove(0);
        let previous = crate::client::adapters::tmux::require_live_session_name(&endpoint).unwrap();
        let parent = server.host_paths.state_root();
        std::fs::create_dir_all(parent).unwrap();
        let permissions = std::fs::metadata(parent).unwrap().permissions();
        std::fs::set_permissions(parent,std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = manager.publish_register_control_before_consume(&server,&prepared,None,None,None);
        std::fs::set_permissions(parent,permissions).unwrap();
        let error = result.unwrap_err();
        assert!(error.starts_with("REGISTER_CREDENTIAL_STAGE_FAILED:"),"{error}");
        assert_eq!(crate::client::adapters::tmux::require_live_session_name(&endpoint).unwrap(),previous);
        assert!(!parent.join("identity-map.json").exists());
        assert!(!parent.join("identities/rename-owner/identity.json").exists());
        assert!(server.state.lock().unwrap().workers.is_empty());
        drop(tmux);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn foreign_current_agent_does_not_skip_target_credential_before_active_publication() {
        for foreign_app in [false, true] {
            let (manager, server, root, tmux, prepared) = prepared_tmux_registration();
            let TypedCommand::RegisterWorker { worker, binding, .. } = &prepared.typed.command;
            let mut foreign = binding.clone();
            let foreign_root = if foreign_app { root.clone() } else { root.join("foreign-project") };
            std::fs::create_dir_all(&foreign_root).unwrap();
            foreign.project_scope = GlobalState::canonical_project_scope(&foreign_root).unwrap();
            if foreign_app { foreign.app_scope_id = AppServerId::new("foreign-app").unwrap(); }
            let transport = worker.transport.clone().unwrap();
            let key = crate::server::binding_slot::IdentityBindingKey::new(
                foreign.project_scope.clone(), foreign.app_scope_id.clone(), "Alice",
            );
            let mut control = crate::server::binding_slot::BindingControlSet::new();
            control.commit(&key, crate::server::binding_slot::BindingValue::new(
                foreign.clone(), transport.clone()), None).unwrap();
            manager.binding_control_store().save(&control).unwrap();
            let mut credential = crate::identity::Identity {
                worker_id:worker.id.clone(), token:"foreign-scope-token".into(),
                project_scope:None, runtime:None, transport:None,
            };
            crate::identity::persist_current_binding_at(&server.host_paths,
                &Scope { root:foreign_root }, &mut credential,
                crate::identity::RuntimeIdentity {
                    agent_id:foreign.agent_id.clone(), runtime_id:foreign.runtime_id.clone(),
                    appserver_id:foreign.app_scope_id.clone(), binding_id:foreign.binding_id.clone(),
                    session_id:foreign.session_id.clone(), native_thread_id:foreign.native_thread_id.clone(),
                }, transport,
            ).unwrap();
            test_hooks::fail_next_parent_sync();
            let error = manager.publish_register_control_before_consume(
                &server, &prepared, None, None, None,
            ).unwrap_err();
            assert!(error.starts_with("BINDING_CONTROL_DURABILITY_UNKNOWN:"), "{error}");
            let credential = crate::identity::read_persisted(&server.host_paths, &worker.id).unwrap().unwrap();
            assert_eq!(credential.project_scope.as_ref(), Some(&binding.project_scope));
            assert_eq!(credential.runtime.as_ref().unwrap().appserver_id, binding.app_scope_id);
            assert!(credential.token == worker.token, "target credential did not retain the prepared token");
            let control = manager.binding_control_store().load().unwrap();
            assert_eq!(control.slot(&key).unwrap().active_binding().unwrap().binding, foreign);
            assert_eq!(control.active_values().count(),2);
            assert!(server.state.lock().unwrap().workers.is_empty());
            drop(tmux);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn register_control_unknown_keeps_the_new_name_and_unconsumed_intent() {
        let (manager, server, root, tmux, prepared) = prepared_tmux_registration();
        let endpoint = tmux.endpoints().remove(0);
        let previous = crate::client::adapters::tmux::require_live_session_name(&endpoint).unwrap();
        test_hooks::fail_next_parent_sync();
        let error = manager.publish_register_control_before_consume(&server, &prepared, None, None, None).unwrap_err();
        assert!(error.starts_with("BINDING_CONTROL_DURABILITY_UNKNOWN:"), "{error}");
        let map = manager.binding_control_store().load().unwrap();
        let values: Vec<_> = map.active_values().collect();
        assert_eq!(values.len(), 1);
        let name = crate::client::adapters::tmux::require_live_session_name(&endpoint).unwrap();
        assert_eq!(name, previous, "no manifest target was declared for this pane");
        assert_eq!(values[0].physical_name.as_deref(), Some(name.as_str()));
        assert_eq!(map.pending_effects().count(), 1);
        let credential = crate::identity::read_persisted(
            &server.host_paths, values[0].binding.agent_id.as_str(),
        ).unwrap();
        assert!(credential.is_some(), "active current was published before its exact credential");
        assert!(server.state.lock().unwrap().workers.is_empty(), "Unknown cannot consume business Register");
        drop(tmux);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_runtime_pane_replacement_preserves_an_exact_active_other_principal() {
        let (manager, server, root, tmux, first) = prepared_tmux_registration();
        let TypedCommand::RegisterWorker { worker, binding, .. } = &first.typed.command;
        manager.install_runtime(&(
            binding.app_scope_id.as_str().to_owned(),
            binding.project_scope.as_str().to_owned(),
        ), server.clone(), None);
        let mut transport = worker.transport.clone().unwrap();
        let first_effect = manager.publish_register_control_before_consume(&server, &first, None, None, None).unwrap();
        server.consume_register(first, None).unwrap();
        manager.consume_register_effect(Some(&first_effect)).unwrap();
        let before = server.state.lock().unwrap().workers["rename-owner"].clone();
        let map = manager.binding_control_store().load().unwrap();
        let binding = map.active_values().next().unwrap().binding.clone();
        let native = peer_tests::test_appserver_transport("other-scope-thread");
        assert!(peer_tests::seed_committed_owner(&server, "other-scope-peer", "other-scope-token",
            &native, root.to_str().unwrap(), Some(AppServerId::new("tui-default").unwrap())).ok);
        transport.session_id = Some("other-scope-session".into());
        transport.thread_id = Some("other-scope-thread".into());
        let endpoint = transport.tmux_endpoint.as_mut().unwrap();
        endpoint.codex_session_id = transport.session_id.clone();
        endpoint.codex_thread_id = transport.thread_id.clone();
        let second = server.prepare_register_envelope_for_scope(
            "other-scope-peer", "other-scope-token", &transport,
            GlobalState::canonical_project_scope(&root).unwrap(), root.to_str().unwrap(),
            AppServerId::new("tui-default").unwrap(), false,
        ).unwrap();
        let second_effect = manager.publish_register_control_before_consume(&server, &second, None, None, None).unwrap();
        server.consume_register(second, None).unwrap();
        manager.consume_register_effect(Some(&second_effect)).unwrap();
        let state = server.state.lock().unwrap();
        assert_eq!(serde_json::to_value(&state.workers["rename-owner"]).unwrap(), serde_json::to_value(before).unwrap());
        assert_eq!(state.global.lookup_binding(&binding.binding_id), Some(&binding));
        assert!(state.workers.contains_key("other-scope-peer"));
        assert_eq!(state.global.lookup_unique_tmux_pane_route(transport.tmux_endpoint.as_ref().unwrap())
            .map(|route| route.agent_id.as_str()), Some("other-scope-peer"));
        drop(state);
        assert!(manager.binding_control_store().load().unwrap().holds_current_binding(&binding));
        drop(tmux);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn outgoing_name_effect_waits_for_the_primary_handoff_without_changing_business_state() {
        let (server, root) = peer_tests::test_server();
        let server = Arc::new(server);
        let app = AppServerId::new("tui-default").unwrap();
        for (worker, thread) in [("first", "first-thread"), ("second", "second-thread")] {
            assert!(peer_tests::seed_committed_owner(&server, worker, "fixture-token",
                &peer_tests::test_appserver_transport(thread), root.to_str().unwrap(), Some(app.clone())).ok);
        }
        let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
        let map = manager.binding_control_store().load().unwrap();
        let scope = GlobalState::canonical_project_scope(&root).unwrap();
        let alice = binding_slot::IdentityBindingKey::new(scope.clone(), app.clone(), "Alice");
        let bob = binding_slot::IdentityBindingKey::new(scope.clone(), app.clone(), "Bob");
        let first = map.slot(&alice).unwrap().active_binding().unwrap().clone();
        let second = map.slot(&bob).unwrap().active_binding().unwrap().clone();
        let primary = binding_slot::PendingEffect {
            kind: binding_slot::PendingEffectKind::Name, role: None,
            effect_id: "name:pending-owner".into(), operation_id: "pending-owner".into(),
            project_scope: scope, app_scope_id: app, name: "Alice".into(),
            previous_worker_id: Some("first".into()), previous_session_id: first.binding.session_id.as_ref().map(ToString::to_string),
            target_worker_id: "second".into(), target_session_id: second.binding.session_id.as_ref().unwrap().to_string(),
            occupancy_revision: 2, source_names: vec!["Bob".into()], moves_existing_names: true,
            pane_anchor: None, created_ms: now_ms(),
        };
        let outgoing = binding_slot::PendingEffect {
            effect_id: "name:pending-owner:outgoing".into(), name: "Bob".into(),
            previous_worker_id: None, previous_session_id: None,
            target_worker_id: "first".into(), target_session_id: first.binding.session_id.as_ref().unwrap().to_string(),
            source_names: Vec::new(), moves_existing_names: false, ..primary.clone()
        };
        manager.commit_control_change(binding_store::ControlChange::MoveNameWithEffect {
            target: alice, value: second, source_names: vec!["Bob".into()], expected_generation: None,
            effect: primary, companion: Some((bob, first, outgoing.clone())),
        }).unwrap();
        let before = std::fs::read(&server.state_path).unwrap();
        let error = manager.consume_register_effect(Some(&outgoing.effect_id)).unwrap_err();
        assert_eq!(error, "PENDING_EFFECT_PRIMARY_INCOMPLETE: name:pending-owner");
        assert_eq!(std::fs::read(&server.state_path).unwrap(), before);
        let map = manager.binding_control_store().load().unwrap();
        assert!(map.pending_effect("name:pending-owner").is_some());
        assert!(map.pending_effect(&outgoing.effect_id).is_some());
        assert_eq!(map.active_values().count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn prepared(server: &Server, root: &Path, worker: &str) -> PreparedRegisterEnvelope {
        let transport = SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/scoped-register.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some(format!("session-{worker}")),
            thread_id: Some(format!("thread-{worker}")),
            tmux_endpoint: None,
            rex_endpoint: None,
            capabilities: Vec::new(),
            self_check: "owner test".into(),
        };
        server
            .prepare_register_envelope_for_scope(
                worker,
                "owner-test-token",
                &transport,
                GlobalState::canonical_project_scope(root).unwrap(),
                root.to_str().unwrap(),
                AppServerId::new("tui-default").unwrap(),
                false,
            )
            .unwrap()
    }

    #[test]
    fn prepared_registration_survives_an_unrelated_ledger_scan() {
        let (server, root) = peer_tests::test_server();
        let registration = prepared(&server, &root, "scan-independent-peer");
        server.commit_checked(&[Event::GlobalLedgerScanReceiptRecorded {
            receipt: global_state::LedgerScanReceipt {
                scan_id: "unrelated-register-scan".into(), scanned_ms: now_ms(),
                classified: 0, transitioned: 0, unchanged: 0, blocked: 0,
                mailbox_messages_unchanged: true,
            },
        }]).unwrap();
        server.typed_dispatch(registration.typed).expect("a scan cannot invalidate authorized registration");
        assert!(server.state.lock().unwrap().workers.contains_key("scan-independent-peer"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn scoped_register_ids_separate_scopes_but_not_changed_intent() {
        let (server, root) = peer_tests::test_server();
        let first = prepared(&server, &root, "scope-key-worker");
        let TypedCommand::RegisterWorker { worker, .. } = &first.typed.command;
        let transport = worker.transport.as_ref().unwrap();
        let prepare = |project, app, token| {
            server
                .prepare_register_envelope_for_scope(
                    &worker.id,
                    token,
                    transport,
                    project,
                    &worker.cwd,
                    app,
                    false,
                )
                .unwrap()
        };
        let scope = &first.typed.envelope.scope;
        let other_app = prepare(
            scope.project_scope_id.clone(),
            AppServerId::new("another-app").unwrap(),
            &worker.token,
        );
        let other_project = prepare(
            ProjectScopeId::new("/another-project").unwrap(),
            scope.app_scope_id.clone(),
            &worker.token,
        );
        let changed_intent = prepare(
            scope.project_scope_id.clone(),
            scope.app_scope_id.clone(),
            "different-token",
        );
        for other in [other_app, other_project] {
            assert_ne!(first.nested_command_id, other.nested_command_id);
            assert_ne!(first.nested_operation_id, other.nested_operation_id);
            assert!(other.nested_command_id.len() < 256);
            assert!(other.nested_operation_id.len() < 256);
        }
        assert_eq!(first.nested_command_id, changed_intent.nested_command_id);
        assert_eq!(
            first.nested_operation_id,
            changed_intent.nested_operation_id
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Build one registered project State that already holds `old` as the
    /// exact runtime binding for `runtime_id`.
    fn state_holding_runtime(old: &RuntimeBinding) -> State {
        let mut state = State::default();
        state
            .global
            .register_project(ProjectRegistration::new(
                old.project_scope.clone(),
                old.app_scope_id.clone(),
            )
            .unwrap())
            .unwrap();
        state
            .global
            .bind_runtime(old.clone())
            .expect("seed the prior runtime binding");
        state
    }

    /// Register one more current principal that still occupies `runtime_id`.
    fn commit_runtime_current(
        manager: &ProjectRuntimeManager,
        transport: &SelectedTransport,
        binding: &RuntimeBinding,
        name: &str,
    ) {
        let key = crate::server::binding_slot::IdentityBindingKey::new(
            binding.project_scope.clone(),
            binding.app_scope_id.clone(),
            name,
        );
        let mut control = manager.binding_control_store().load().unwrap();
        control
            .commit(
                &key,
                crate::server::binding_slot::BindingValue::new(
                    binding.clone(),
                    transport.clone(),
                ),
                None,
            )
            .unwrap();
        manager.binding_control_store().save(&control).unwrap();
    }

    fn prepared_incoming_register(
        old: &RuntimeBinding,
        worker_id: &str,
    ) -> RuntimeBinding {
        RuntimeBinding {
            agent_id: AgentId::new(worker_id).unwrap(),
            binding_id: BindingId::new(format!("binding-{worker_id}")).unwrap(),
            ..old.clone()
        }
    }

    /// The current map still protects the prior principal while its State
    /// binding is still the exact old generation. The plan must use the full
    /// principal relation, so an exact-equality-only check would wrongly
    /// release a principal the map still owns.
    #[test]
    fn release_planner_refuses_a_current_that_still_protects_the_prior_principal() {
        let (_, server, root, tmux, prepared) = prepared_tmux_registration();
        let TypedCommand::RegisterWorker { worker, binding, .. } = &prepared.typed.command;
        let old = binding.clone();
        let state = state_holding_runtime(&old);
        let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
        let transport = worker.transport.clone().unwrap();
        commit_runtime_current(&manager, &transport, &old, "Alice");

        let later = RuntimeBinding {
            agent_id: old.agent_id.clone(),
            binding_id: BindingId::new("binding-role-current-next").unwrap(),
            session_id: Some(SessionId::new("role-current-next-session").unwrap()),
            native_thread_id: old.native_thread_id.clone(),
            ..old.clone()
        };
        commit_runtime_current(&manager, &transport, &later, "Alice");
        let control = manager.binding_control_store().load().unwrap();
        assert_eq!(control.active_values().count(), 1);
        assert!(!control.holds_current_binding(&old));
        assert!(control.holds_current_binding(&later));

        let incoming = prepared_incoming_register(&old, "role-current-intruder");
        let error = server
            .register_execution_release_events(&state, &control, &incoming)
            .expect_err("a current map that still protects the prior principal must block release");
        assert!(
            error.contains("current map still protects the prior principal"),
            "{error}"
        );

        drop(tmux);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A current principal that still occupies the same runtime blocks any
    /// release, even when it is a different principal from the incoming one.
    #[test]
    fn release_planner_refuses_another_current_on_the_same_runtime() {
        let (_, server, root, tmux, prepared) = prepared_tmux_registration();
        let TypedCommand::RegisterWorker { worker, binding, .. } = &prepared.typed.command;
        let old = binding.clone();
        let state = state_holding_runtime(&old);
        let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
        let transport = worker.transport.clone().unwrap();
        let other = RuntimeBinding {
            agent_id: AgentId::new("role-runtime-other").unwrap(),
            binding_id: BindingId::new("binding-role-runtime-other").unwrap(),
            ..old.clone()
        };
        commit_runtime_current(&manager, &transport, &other, "Alice");
        let control = manager.binding_control_store().load().unwrap();
        assert!(control.active_values().all(|current| current.binding != old));
        assert!(control.holds_current_binding(&other));

        let incoming = prepared_incoming_register(&old, "role-runtime-incoming");
        let error = server
            .register_execution_release_events(&state, &control, &incoming)
            .expect_err("another current owner of the same runtime must block release");
        assert!(
            error.contains("another current still owns the runtime"),
            "{error}"
        );

        drop(tmux);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn typed_register_keeps_runtime_conflict_without_a_releasable_current() {
        let (_, server, root, tmux, prepared) = prepared_tmux_registration();
        let TypedCommand::RegisterWorker { worker, binding, .. } = &prepared.typed.command;
        let old = binding.clone();
        {
            let mut state = server.state.lock().unwrap();
            state
                .global
                .register_project(ProjectRegistration::new(
                    old.project_scope.clone(),
                    old.app_scope_id.clone(),
                )
                .unwrap())
                .unwrap();
            state.global.bind_runtime(old.clone()).unwrap();
        }

        let transport = worker.transport.clone().unwrap();
        let manager = ProjectRuntimeManager::new(server.clone(), &server.host_paths).unwrap();
        commit_runtime_current(&manager, &transport, &old, "Alice");
        let control = manager.binding_control_store().load().unwrap();
        let mut incoming = server
            .prepare_register_envelope_for_scope(
                "role-runtime-incoming",
                "role-runtime-incoming-token",
                &transport,
                old.project_scope.clone(),
                root.to_str().unwrap(),
                old.app_scope_id.clone(),
                false,
            )
            .unwrap();
        if let TypedCommand::RegisterWorker { binding, .. } = &mut incoming.typed.command {
            binding.runtime_id = old.runtime_id.clone();
        }
        let before_state = server.state.lock().unwrap().clone();
        let before_map = manager.binding_control_store().load().unwrap();

        let protected = server.typed_dispatch_register_with_origin(
            incoming.typed.clone(),
            None,
            RegisterOrigin::Registration,
            Some(&control),
        );
        assert!(
            protected
                .unwrap_err()
                .to_string()
                .contains("current map still protects the prior principal"),
            "a current-map principal cannot be released by another Register"
        );
        assert_eq!(
            serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
            serde_json::to_value(&before_state).unwrap()
        );
        assert_eq!(
            serde_json::to_value(manager.binding_control_store().load().unwrap()).unwrap(),
            serde_json::to_value(&before_map).unwrap()
        );

        let unprotected = server.typed_dispatch_register_with_origin(
            incoming.typed.clone(),
            None,
            RegisterOrigin::Registration,
            None,
        );
        assert!(
            unprotected
                .unwrap_err()
                .to_string()
                .contains("already bound as"),
            "ordinary typed Register still preserves strong runtime uniqueness"
        );
        assert_eq!(
            serde_json::to_value(&*server.state.lock().unwrap()).unwrap(),
            serde_json::to_value(&before_state).unwrap()
        );
        assert_eq!(
            serde_json::to_value(manager.binding_control_store().load().unwrap()).unwrap(),
            serde_json::to_value(&before_map).unwrap()
        );

        drop(tmux);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The release event removes exactly one runtime binding. It must not touch
    /// the worker, tasks, messages, or subscriptions that still own their work.
    #[test]
    fn runtime_execution_release_keeps_worker_and_business_state() {
        let (_, _server, root, tmux, prepared) = prepared_tmux_registration();
        let TypedCommand::RegisterWorker { worker, binding, .. } = &prepared.typed.command;
        let old = binding.clone();
        let mut state = state_holding_runtime(&old);
        state.workers.insert(worker.id.clone(), worker.clone());
        let scope = old.project_scope.as_str().to_owned();
        state
            .msgs
            .insert(
                "release-keeps-message".into(),
                crate::server::Message {
                    id: "release-keeps-message".into(),
                    from: worker.id.clone(),
                    to: worker.id.clone(),
                    mtype: "notify".into(),
                    subject: None,
                    body: "keep".into(),
                    state: "pending".into(),
                    created_ms: now_ms(),
                    in_reply_to: None,
                    wake_attempt_count: 0,
                    last_wake_attempt_ms: 0,
                    retry_attempted: false,
                },
            );
        let before_workers = state.workers.clone();
        let before_msgs = state.msgs.clone();
        state
            .apply_checked(&Event::GlobalRuntimeExecutionReleased {
                binding: old.clone(),
            })
            .unwrap();
        assert_eq!(
            state.global.lookup_binding(&old.binding_id),
            None,
            "the exact released binding is gone"
        );
        assert_eq!(
            serde_json::to_value(&state.workers).unwrap(),
            serde_json::to_value(&before_workers).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&state.msgs).unwrap(),
            serde_json::to_value(&before_msgs).unwrap()
        );
        assert!(
            state
                .global
                .lookup_project(&old.project_scope)
                .is_some(),
            "the project and its registration stay: {scope}"
        );
        drop(tmux);
        std::fs::remove_dir_all(root).unwrap();
    }

}
