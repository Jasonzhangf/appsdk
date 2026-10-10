#[cfg(test)]
#[path = "../lifecycle_fence_tests.rs"]
mod lifecycle_fence_tests;

thread_local! {
    static CONTEXT_CANCEL_OPERATION: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
    static CONTEXT_REGISTER_START_BINDING: std::cell::RefCell<Option<RegisterOuterBinding>> =
        const { std::cell::RefCell::new(None) };
}

fn set_context_cancel_operation(operation_id: &str) {
    CONTEXT_CANCEL_OPERATION.with(|value| {
        *value.borrow_mut() = Some(operation_id.to_owned());
    });
}

pub(crate) fn clear_context_cancel_operation() {
    CONTEXT_CANCEL_OPERATION.with(|value| {
        *value.borrow_mut() = None;
    });
}

fn take_context_cancel_operation() -> Option<String> {
    CONTEXT_CANCEL_OPERATION.with(|value| value.borrow_mut().take())
}

fn set_context_register_start_binding(binding: &RegisterOuterBinding) {
    CONTEXT_REGISTER_START_BINDING.with(|value| {
        *value.borrow_mut() = Some(binding.clone());
    });
}

fn take_context_register_start_binding() -> Option<RegisterOuterBinding> {
    CONTEXT_REGISTER_START_BINDING.with(|value| value.borrow_mut().take())
}

pub(crate) fn clear_context_register_start_binding() {
    CONTEXT_REGISTER_START_BINDING.with(|value| {
        value.borrow_mut().take();
    });
}

/// A Register command prepared from one consistent reducer snapshot.
///
/// The nested `command_id`/`operation_id` in `typed` are the durable outer
/// identifiers bound before consume. `consume_prepared_register` must send
/// this exact envelope; it never regenerates the identifiers.
pub(crate) struct PreparedRegisterEnvelope {
    pub(crate) nested_command_id: String,
    pub(crate) nested_operation_id: String,
    pub(crate) typed: TypedEnvelope,
}

/// Daemon-issued admission proof for one approved stale-credential recovery.
///
/// The identity owner mints this only after it has validated the exact user
/// approval against the committed incumbent. It is not a token, a user-visible
/// credential, or a fabricated `actor_binding_id`: the Register owner still
/// commits the real binding it derives. The proof only lets the Register owner
/// admit the approved target when the ordinary token check would reject a
/// stale incumbent credential, and only for the exact target/scope/generation
/// this proof names. A public `Req::Register` never carries one.
#[derive(Debug, Clone)]
pub(crate) struct RegisterApprovalProof {
    pub(crate) target_identity: String,
    pub(crate) project_scope: String,
    pub(crate) app_scope_id: String,
    pub(crate) incumbent_binding_id: String,
    pub(crate) incumbent_endpoint_generation: u64,
}

impl RegisterApprovalProof {
    /// True only when the target and route scope name exactly this proof's
    /// approved target and scope.
    pub(crate) fn authorizes(
        &self,
        worker_id: &str,
        project_scope: &str,
        app_scope_id: &str,
    ) -> bool {
        self.target_identity == worker_id
            && self.project_scope == project_scope
            && self.app_scope_id == app_scope_id
    }
}

/// Host outer-operation binder: it durably records a prepared Register
/// envelope's nested IDs in the outer journal before that envelope is
/// consumed, then exposes the exact bound IDs to the caller. It holds only the
/// shared journal, the admitted outer operation key, and the bound IDs, never
/// identity secret material.
#[derive(Clone)]
pub(crate) struct RegisterOuterBinding {
    journal: Arc<crate::server::operation_journal::OperationJournal>,
    operation_id: String,
    approval: Option<Arc<RegisterApprovalProof>>,
    initial_business_receipts: Vec<String>,
    bound: Arc<Mutex<Option<(String, String)>>>,
}

impl RegisterOuterBinding {
    pub(crate) fn new(
        journal: Arc<crate::server::operation_journal::OperationJournal>,
        operation_id: String,
    ) -> Self {
        Self {
            journal,
            operation_id,
            approval: None,
            initial_business_receipts: Vec::new(),
            bound: Arc::new(Mutex::new(None)),
        }
    }

    /// Construct the binder for one approved stale-credential recovery. The
    /// proof travels with the same durable outer operation that binds the
    /// nested Register ids.
    pub(crate) fn with_approval(
        journal: Arc<crate::server::operation_journal::OperationJournal>,
        operation_id: String,
        approval: RegisterApprovalProof,
    ) -> Self {
        Self {
            journal,
            operation_id,
            approval: Some(Arc::new(approval)),
            // The approval is immutable audit evidence on the durable
            // admission record. It becomes a public business receipt only
            // after the exact nested Register `CommandStarted` has synced.
            initial_business_receipts: Vec::new(),
            bound: Arc::new(Mutex::new(None)),
        }
    }

    /// The approved-recovery proof for this operation, if any. Ordinary
    /// automatic/supplement operations never carry one.
    pub(crate) fn approval(&self) -> Option<&RegisterApprovalProof> {
        self.approval.as_deref()
    }

    /// Append and sync the outer `Validating` phase with the exact nested IDs
    /// of `prepared`. On any failure the bound state stays `None` and durable
    /// state is unchanged, so the caller must not consume.
    pub(crate) fn bind_validating(
        &self,
        prepared: &PreparedRegisterEnvelope,
    ) -> Result<(), String> {
        self.journal
            .transition_with_business_receipts(
                &self.operation_id,
                crate::proto::IdentityOperationPhase::Validating,
                Some(prepared.nested_command_id.clone()),
                Some(prepared.nested_operation_id.clone()),
                Some(self.initial_business_receipts.clone()),
            )
            .map(|_| {
                *self.bound.lock().unwrap() = Some((
                    prepared.nested_command_id.clone(),
                    prepared.nested_operation_id.clone(),
                ));
            })
    }

    pub(crate) fn bound(&self) -> Option<(String, String)> {
        self.bound.lock().unwrap().clone()
    }

    /// Promote the outer `InnerDispatched` transition after the nested
    /// Register owner has durably started. Approved recovery records the
    /// `approval_decision` receipt at this exact point.
    pub(crate) fn promote_inner_dispatched(&self) -> Result<(), String> {
        let Some((command_id, operation_id)) = self.bound() else {
            return Err("IDENTITY_OPERATION_PHASE_CONFLICT: no nested receipt is bound".into());
        };
        let receipts = self.initial_business_receipts.clone();
        self.journal
            .transition_with_business_receipts(
                &self.operation_id,
                crate::proto::IdentityOperationPhase::InnerDispatched,
                Some(command_id),
                Some(operation_id),
                Some(receipts),
            )
            .map(|_| ())
    }

    /// One arbitration decision shared with cancellation. It marks the owner
    /// active before the first effect-producing call. The durable Start
    /// append follows immediately in the same consume path.
    pub(crate) fn arbitrate_owner_start(&self) -> Result<(), String> {
        set_context_cancel_operation(&self.operation_id);
        self.journal
            .begin_owner_start(&self.operation_id, "register")
    }

    pub(crate) fn promote_after_start(&self) -> Result<(), String> {
        set_context_register_start_binding(self);
        Ok(())
    }
}

impl Server {
    pub(crate) fn log_path_for(root: &Path) -> PathBuf {
        root.join(".agent-collab").join("server").join("log.txt")
    }

    pub fn log_path(&self) -> PathBuf {
        Self::log_path_for(&self.storage_root)
    }

    pub(crate) fn storage_server_dir(&self) -> PathBuf {
        self.storage_root.join(".agent-collab").join("server")
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
        let binding_text = sanitize_identifier(&format!("binding-{worker_id}"));
        let binding_id = BindingId::new(binding_text.clone()).map_err(|error| error.to_string())?;
        let route_scope = RouteScope {
            app_scope_id: app_scope.clone(),
            project_scope_id: project_scope.clone(),
        };
        // All state-dependent ID inputs and the actual binding are derived
        // from one snapshot. The CAS is still rechecked by the reducer at
        // consume time, so revision changes after prepare are rejected.
        let (
            runtime_text,
            generation,
            registration,
            registered_ms,
            expected_revision,
            base_command,
            prior_attempt,
        ) = {
            let st = self.state.lock().unwrap();
            let (runtime_text, generation) =
                match st.global.lookup_binding_for(&route_scope, &binding_id) {
                    Some(existing) if existing.agent_id.as_str() == worker_id => {
                        let runtime_text = existing.runtime_id.as_str().to_owned();
                        let generation = if reuse_existing {
                            existing.endpoint_generation
                        } else {
                            existing
                                .endpoint_generation
                                .checked_add(1)
                                .ok_or_else(|| "endpoint generation overflow".to_string())?
                        };
                        (runtime_text, generation)
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
                                    let endpoint = transport.tmux_endpoint.as_ref().ok_or_else(|| {
                                "RUNTIME_BINDING_REJECTED: tmux transport has no endpoint"
                                    .to_string()
                            })?;
                                    let bytes = serde_json::to_vec(endpoint).map_err(|error| {
                                format!("RUNTIME_BINDING_REJECTED: encode tmux endpoint: {error}")
                            })?;
                                    let fingerprint = bytes.into_iter().fold(
                                        0xcbf29ce484222325_u64,
                                        |mut hash, byte| {
                                            hash ^= u64::from(byte);
                                            hash.wrapping_mul(0x100000001b3)
                                        },
                                    );
                                    format!("runtime-tmux-{fingerprint:016x}")
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
                        (runtime_text, 1)
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
            let base_command = format!("register-{binding_text}-{generation}");
            // `prior_attempt` selects the retry suffix, so it must be read in
            // the same snapshot as `generation` and `expected_revision`.
            let prior_attempt = !reuse_existing
                && st
                    .global
                    .lookup_command_receipt(
                        &CommandId::new(base_command.clone()).map_err(|error| error.to_string())?,
                    )
                    .is_some();
            (
                runtime_text,
                generation,
                registration,
                registered_ms,
                st.revision,
                base_command,
                prior_attempt,
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
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "App Server registration requires session_id".to_string())?;
        let session_id = Some(
            crate::identity::SessionId::new(session_id.clone())
                .map_err(|error| error.to_string())?,
        );
        let mut binding = RuntimeBinding::new_with_session(
            project_scope.clone(),
            app_scope,
            agent_id,
            runtime_id,
            binding_id.clone(),
            generation,
            session_id,
            native_thread_id,
        )
        .map_err(|error| error.to_string())?;
        binding.tmux_endpoint = transport.tmux_endpoint.clone();
        binding.validate().map_err(|error| error.to_string())?;
        let attempt_suffix = prior_attempt
            .then(|| format!("-retry-{expected_revision}"))
            .unwrap_or_default();
        let command_id = CommandId::new(format!("{base_command}{attempt_suffix}"))
            .map_err(|error| error.to_string())?;
        let operation_id = OperationId::new(format!(
            "register-op-{binding_text}-{generation}{attempt_suffix}"
        ))
        .map_err(|error| error.to_string())?;
        let nested_command_id = command_id.as_str().to_owned();
        let nested_operation_id = operation_id.as_str().to_owned();
        let envelope = CommandEnvelope::new(
            command_id,
            operation_id,
            binding_id,
            generation,
            route_scope,
            Some(expected_revision),
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

    fn consume_prepared_register(
        &self,
        prepared: PreparedRegisterEnvelope,
    ) -> Result<state::TypedOutcome, notification_contract::JournalError> {
        self.typed_dispatch_with_approval(prepared.typed, None, None)
    }

    fn consume_prepared_register_with_approval(
        &self,
        prepared: PreparedRegisterEnvelope,
        approval: Option<&RegisterApprovalProof>,
    ) -> Result<state::TypedOutcome, notification_contract::JournalError> {
        self.typed_dispatch_with_approval(prepared.typed, approval, None)
    }

    /// Dispatch one typed command through validation, journal append, flush
    /// and reducer apply. This is the production typed seam above the legacy
    /// CLI adapters; the legacy v1 call site routes through it.
    pub fn typed_dispatch(
        &self,
        typed: TypedEnvelope,
    ) -> Result<state::TypedOutcome, notification_contract::JournalError> {
        self.typed_dispatch_with_approval(typed, None, None)
    }

    fn typed_dispatch_with_approval(
        &self,
        typed: TypedEnvelope,
        approval: Option<&RegisterApprovalProof>,
        outer_binding: Option<&RegisterOuterBinding>,
    ) -> Result<state::TypedOutcome, notification_contract::JournalError> {
        typed.envelope.validate().map_err(|error| {
            notification_contract::JournalError::InvalidCommand(error.to_string())
        })?;
        let mut st = self.state.lock().unwrap();
        self.validate_typed_register(&st, &typed, approval)?;
        let mut events = Vec::new();
        let TypedCommand::RegisterWorker { worker, .. } = &typed.command;
        for global_event in typed.command.global_events() {
            match global_event {
                GlobalEvent::ProjectRegistered { registration } => {
                    events.push(Event::GlobalProjectRegistered { registration })
                }
                GlobalEvent::RuntimeBound { binding } => {
                    let previous = st
                        .global
                        .lookup_binding_for(&binding.route_scope(), &binding.binding_id);
                    let replaces_generation = previous.is_some_and(|current| {
                        current.same_principal(&binding)
                            && current.endpoint_generation < binding.endpoint_generation
                    });
                    let same_pane_tmux_recovery = replaces_generation
                        && st
                            .global
                            .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
                            .is_some()
                        && previous.is_some_and(|current| {
                            current.tmux_endpoint.as_ref().is_some_and(|old| {
                                binding.tmux_endpoint.as_ref().is_some_and(|new| {
                                    crate::client::adapters::tmux::same_owned_pane(old, new)
                                })
                            })
                        })
                        && st.workers.get(&worker.id).is_some_and(|old| {
                            selected_transport_for_worker(old)
                                .is_some_and(|transport| transport.kind == TransportKind::Tmux)
                        })
                        && selected_transport_for_worker(worker)
                            .is_some_and(|transport| transport.kind == TransportKind::Tmux);
                    // A same-principal generation replacement is the
                    // reconnect/recovery path, so the capability fence must
                    // reissue the previous grant for the new generation in
                    // this same transaction.  An unrelated binding never
                    // carries a previous grant here, so its reconnect cannot
                    // touch the current master.
                    let reissued_master_grant = previous
                        .filter(|_| replaces_generation)
                        .and_then(|current| {
                            st.global
                                .lookup_master_grant_for(
                                    &binding.route_scope(),
                                    &binding.binding_id,
                                )
                                .filter(|grant| {
                                    grant.endpoint_generation == current.endpoint_generation
                                        && grant.project_scope == binding.project_scope
                                        && grant.app_scope_id == binding.app_scope_id
                                        && grant.agent_id == binding.agent_id
                                        && grant.binding_id == binding.binding_id
                                })
                                .cloned()
                        })
                        .map(|mut grant| {
                            grant.endpoint_generation = binding.endpoint_generation;
                            grant
                        });
                    if replaces_generation {
                        let legacy_master_replaced = reissued_master_grant.is_none()
                            && st.master_worker_id.as_ref().is_some_and(|worker_id| {
                                worker_id.as_str() == binding.agent_id.as_str()
                            });
                        if legacy_master_replaced {
                            events.push(Event::GlobalMasterRevoked {
                                project_scope: binding.project_scope.clone(),
                                binding_id: binding.binding_id.clone(),
                            });
                        }
                    }
                    // A pane has one owner. The later registrant therefore
                    // replaces the previous claimant of this pane in this same
                    // commit, so the ledger never holds two owners of one pane
                    // and cannot fence the new owner on the next attempt.
                    if let Some(endpoint) = binding.tmux_endpoint.as_ref() {
                        for claimant in pane_claimants(
                            &st,
                            binding.agent_id.as_str(),
                            &binding.route_scope(),
                            endpoint,
                        ) {
                            events.extend(
                                pane_reclaim_events(binding.agent_id.as_str(), &claimant)
                                    .map_err(notification_contract::JournalError::InvalidCommand)?,
                            );
                        }
                    }
                    events.push(Event::GlobalRuntimeBound {
                        binding: binding.clone(),
                    });
                    if let Some(grant) = reissued_master_grant {
                        events.push(Event::GlobalMasterGranted { grant });
                    }
                    if same_pane_tmux_recovery {
                        events.push(Event::GlobalCurrentThreadRouteSet { binding });
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
                GlobalEvent::MigrationCommitEvidence { .. } => {
                    return Err(notification_contract::JournalError::InvalidCommand(
                        "migration commit evidence is not part of worker registration".into(),
                    ))
                }
                GlobalEvent::RuntimeBindingLedgerClassified { record } => {
                    events.push(Event::GlobalRuntimeBindingLedgerClassified { record })
                }
                GlobalEvent::LedgerScanReceiptRecorded { receipt } => {
                    events.push(Event::GlobalLedgerScanReceiptRecorded { receipt })
                }
                GlobalEvent::MasterGrantReplacementStarted { .. }
                | GlobalEvent::MasterGrantReplacementCompleted { .. } => {
                    return Err(notification_contract::JournalError::InvalidCommand(
                        "grant replacement evidence is not part of worker registration".into(),
                    ))
                }
            }
        }
        events.push(Event::Registered {
            worker: worker.clone(),
        });
        if let Some(transport) = selected_transport_for_worker(worker) {
            events.extend(default_direct_message_events(
                &st,
                &worker.id,
                &transport,
                now_ms(),
            ));
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
            outer_binding,
        )?;
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
        approval: Option<&RegisterApprovalProof>,
    ) -> Result<(), notification_contract::JournalError> {
        let TypedCommand::RegisterWorker {
            registration,
            binding,
            worker,
        } = &typed.command;
        if binding.agent_id.as_str() != worker.id {
            return Err(notification_contract::JournalError::InvalidCommand(
                "runtime binding agent does not match worker identity".into(),
            ));
        }
        if registration.route_scope() != binding.route_scope() {
            return Err(notification_contract::JournalError::InvalidCommand(
                "project registration scope does not match runtime binding scope".into(),
            ));
        }
        if let Some(existing) = st.workers.get(&worker.id) {
            if existing.token != worker.token {
                let existing_route_scope =
                    existing_route_scope(st, &worker.id).map_err(|response| {
                        notification_contract::JournalError::InvalidCommand(
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
                let approved_incumbent_current = approval.is_some_and(|proof| {
                    proof.authorizes(
                        worker.id.as_str(),
                        binding.project_scope.as_str(),
                        binding.app_scope_id.as_str(),
                    ) && proof.incumbent_binding_id == binding.binding_id.as_str()
                        && existing_binding.is_some_and(|current| {
                            current.agent_id == binding.agent_id
                                && current.binding_id.as_str() == proof.incumbent_binding_id
                                && current.endpoint_generation
                                    == proof.incumbent_endpoint_generation
                        })
                });
                if !same_route || (!same_runtime_thread && !approved_incumbent_current) {
                    return Err(notification_contract::JournalError::InvalidCommand(
                        "worker token does not belong to the registered runtime identity".into(),
                    ));
                }
            }
        }
        if typed.envelope.actor_binding_id != binding.binding_id {
            return Err(notification_contract::JournalError::InvalidCommand(
                format!(
                    "actor binding {} does not match command binding {}",
                    typed.envelope.actor_binding_id, binding.binding_id
                ),
            ));
        }
        if typed.envelope.endpoint_generation != binding.endpoint_generation {
            return Err(notification_contract::JournalError::InvalidCommand(
                format!(
                    "envelope generation {} does not match binding generation {}",
                    typed.envelope.endpoint_generation, binding.endpoint_generation
                ),
            ));
        }
        if typed.envelope.scope != binding.route_scope() {
            return Err(notification_contract::JournalError::InvalidCommand(
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
                return Err(notification_contract::JournalError::InvalidCommand(format!(
                    "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: resident project {} already has a different or ambiguous runtime app owner",
                    registration.project_scope.as_str()
                )));
            }
        }
        let mut next = st.global.clone();
        for event in typed.command.global_events() {
            event.apply(&mut next).map_err(|error| {
                notification_contract::JournalError::InvalidCommand(error.to_string())
            })?;
        }
        next.validate_binding(binding).map_err(|error| {
            notification_contract::JournalError::InvalidCommand(error.to_string())
        })?;
        Ok(())
    }

    /// Apply events to memory and persist them atomically-ordered in the journal.
    ///
    /// Entry point for paths that have no client to answer. A rejected batch is
    /// reported through the explicit failure channel instead of panicking while
    /// the state guard is held; callers that can answer a client use the
    /// checked entry points and propagate the error.
    pub(crate) fn commit(&self, evs: &[Event]) {
        let mut st = self.state.lock().unwrap();
        self.commit_locked_reporting(&mut st, evs);
    }

    /// Fallible reducer entry point used by typed producers.
    pub fn commit_checked(
        &self,
        evs: &[Event],
    ) -> Result<notification_contract::CommitReceipt, notification_contract::JournalError> {
        let mut st = self.state.lock().unwrap();
        self.commit_locked_checked(&mut st, evs)
    }

    /// Compatibility entry point for callers that only need a string error.
    /// The checked reducer remains the single journal/state owner.
    pub(crate) fn try_commit(&self, evs: &[Event]) -> Result<(), String> {
        self.commit_checked(evs)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Compatibility entry point for callers holding the state lock.
    /// This delegates to the typed reducer and never applies state after a
    /// journal failure.
    pub(crate) fn try_commit_locked(&self, st: &mut State, evs: &[Event]) -> Result<(), String> {
        self.commit_locked_checked(st, evs)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Commit one command and its outcome atomically. A retry with the same
    /// command id returns the recorded outcome without appending another event.
    /// Reusing a command id for a different operation is rejected explicitly.
    pub fn commit_command(
        &self,
        command_id: &str,
        operation_id: &str,
        evs: &[Event],
        outcome: serde_json::Value,
    ) -> Result<notification_contract::CommandOutcome, notification_contract::JournalError> {
        validate_command_id(command_id)?;
        validate_command_id(operation_id)?;
        let mut st = self.state.lock().unwrap();
        self.commit_command_locked(&mut st, command_id, operation_id, evs, outcome, None, None)
    }

    fn commit_command_at_revision(
        &self,
        command_id: &str,
        operation_id: &str,
        evs: &[Event],
        outcome: serde_json::Value,
        expected_revision: u64,
    ) -> Result<notification_contract::CommandOutcome, notification_contract::JournalError> {
        validate_command_id(command_id)?;
        validate_command_id(operation_id)?;
        let mut st = self.state.lock().unwrap();
        self.commit_command_locked(
            &mut st,
            command_id,
            operation_id,
            evs,
            outcome,
            Some(expected_revision),
            None,
        )
    }

    fn commit_command_locked(
        &self,
        st: &mut State,
        command_id: &str,
        operation_id: &str,
        evs: &[Event],
        outcome: serde_json::Value,
        expected_revision: Option<u64>,
        outer_binding: Option<&RegisterOuterBinding>,
    ) -> Result<notification_contract::CommandOutcome, notification_contract::JournalError> {
        let typed_command_id = CommandId::new(command_id.to_owned()).map_err(|error| {
            notification_contract::JournalError::InvalidCommand(error.to_string())
        })?;
        if let Some(existing) = st.global.lookup_command_receipt(&typed_command_id) {
            if existing.operation_id.as_str() != operation_id {
                return Err(notification_contract::JournalError::InvalidCommand(
                    format!(
                        "command_id {command_id} already belongs to operation {}",
                        existing.operation_id
                    ),
                ));
            }
            // Same-peer context reuse can name this already-completed Register
            // transaction from a new outer operation. The durable receipt is
            // the owner evidence for that exact nested ID pair; bind it to the
            // new outer operation without writing or replaying Register.
            let scheduled_binding = take_context_register_start_binding();
            let start_binding = outer_binding.or(scheduled_binding.as_ref());
            take_context_cancel_operation();
            if let Some(binding) = start_binding {
                let expected = (command_id.to_owned(), operation_id.to_owned());
                if binding.bound().as_ref() != Some(&expected) {
                    return Err(notification_contract::JournalError::InvalidCommand(
                        "IDENTITY_OPERATION_NESTED_RECEIPT_MISMATCH: committed Register receipt does not match the outer operation binding".into(),
                    ));
                }
                binding
                    .promote_inner_dispatched()
                    .map_err(notification_contract::JournalError::InvalidCommand)?;
            }
            return Ok(notification_contract::CommandOutcome {
                receipt: notification_contract::CommitReceipt {
                    sequence: existing.sequence,
                    revision: existing.revision,
                },
                operation_id: existing.operation_id.as_str().to_owned(),
                outcome: existing.outcome.clone(),
                replayed: true,
            });
        }
        if let Some(existing) = st.command_receipts.get(command_id) {
            if existing.operation_id != operation_id {
                return Err(notification_contract::JournalError::InvalidCommand(
                    format!(
                        "command_id {command_id} already belongs to operation {}",
                        existing.operation_id
                    ),
                ));
            }
            return Ok(notification_contract::CommandOutcome {
                receipt: notification_contract::CommitReceipt {
                    sequence: existing.sequence,
                    revision: existing.revision,
                },
                operation_id: existing.operation_id.clone(),
                outcome: existing.outcome.clone(),
                replayed: true,
            });
        }
        if let Some((existing_command_id, _)) =
            st.global
                .command_receipts
                .iter()
                .find(|(existing_command_id, receipt)| {
                    *existing_command_id != command_id
                        && receipt.operation_id.as_str() == operation_id
                })
        {
            return Err(notification_contract::JournalError::InvalidCommand(
                format!(
                    "operation_id {operation_id} already belongs to command_id {existing_command_id}"
                ),
            ));
        }
        if let Some((existing_command_id, _)) =
            st.command_receipts
                .iter()
                .find(|(existing_command_id, receipt)| {
                    *existing_command_id != command_id && receipt.operation_id == operation_id
                })
        {
            return Err(notification_contract::JournalError::InvalidCommand(
                format!(
                    "operation_id {operation_id} already belongs to command_id {existing_command_id}"
                ),
            ));
        }
        state::responsibility_preflight(st, evs)
            .map_err(notification_contract::JournalError::InvalidCommand)?;
        if let Some(expected_revision) = expected_revision {
            let observed_revision = st.revision;
            if observed_revision != expected_revision {
                return Err(notification_contract::JournalError::InvalidCommand(format!(
                    "compare-and-swap revision mismatch: expected {expected_revision}, observed {observed_revision}"
                )));
            }
        }
        let event_count = evs.len().checked_add(2).ok_or_else(|| {
            notification_contract::JournalError::InvalidCommand(
                "command event count overflow".into(),
            )
        })? as u64;
        let sequence = st.sequence.checked_add(event_count).ok_or_else(|| {
            notification_contract::JournalError::InvalidCommand("sequence counter overflow".into())
        })?;
        let revision = st.revision.checked_add(event_count).ok_or_else(|| {
            notification_contract::JournalError::InvalidCommand("revision counter overflow".into())
        })?;
        let receipt = state::CommandReceipt {
            operation_id: operation_id.to_owned(),
            outcome: outcome.clone(),
            sequence,
            revision,
        };
        let started = Event::CommandStarted {
            command_id: command_id.to_owned(),
            operation_id: operation_id.to_owned(),
        };
        let completed = Event::CommandCompleted {
            command_id: command_id.to_owned(),
            operation_id: operation_id.to_owned(),
            receipt: receipt.clone(),
        };
        self.append_command_phase_locked(
            st,
            std::slice::from_ref(&started),
            CommandJournalPhase::Start,
        )?;
        let scheduled_binding = take_context_register_start_binding();
        let start_binding = outer_binding.or(scheduled_binding.as_ref());
        #[cfg(feature = "context-cancel-test-hooks")]
        if let Some(context_operation_id) = take_context_cancel_operation() {
            let reply = crate::context_cancel_test_hooks::barrier_reply(
                "register_start_synced",
                &context_operation_id,
                std::process::id(),
                Some(command_id),
                Some(operation_id),
                || false,
            );
            if reply.as_deref() == Some("fail_promotion") {
                crate::server::operation_journal::inject_next_append_fault();
            }
        }
        if let Some(binding) = start_binding {
            binding
                .promote_inner_dispatched()
                .map_err(notification_contract::JournalError::InvalidCommand)?;
        }
        self.append_command_phase_locked(st, evs, CommandJournalPhase::Business)?;
        self.append_command_phase_locked(
            st,
            std::slice::from_ref(&completed),
            CommandJournalPhase::Completion,
        )?;
        let mut events = Vec::with_capacity(evs.len() + 2);
        events.push(started);
        events.extend_from_slice(evs);
        events.push(completed);
        self.apply_committed_events(st, &events)?;
        let has_pending_scheduler_admission = events.iter().any(|event| {
            matches!(
                event,
                Event::SchedulerAdmission { admission } if admission.status == "pending"
            )
        });
        let has_succeeded_scheduler_admission = events.iter().any(|event| {
            let Event::SchedulerAdmissionStatus {
                request_id, status, ..
            } = event
            else {
                return false;
            };
            status == "succeeded"
                && st
                    .scheduler_admissions
                    .get(request_id)
                    .is_some_and(|admission| {
                        admission.status == "succeeded"
                            && st.msgs.get(&admission.message_id).is_some_and(|message| {
                                message.state == "pending"
                                    && st.scheduler_message_deliverable(&message.id)
                            })
                    })
        });
        if (events
            .iter()
            .any(|event| matches!(event, Event::Sent { .. }))
            && !has_pending_scheduler_admission)
            || has_succeeded_scheduler_admission
            || events
                .iter()
                .any(|event| matches!(event, Event::GlobalRuntimeBound { .. }))
        {
            self.mailbox_notify.notify_waiters();
        }
        Ok(notification_contract::CommandOutcome {
            receipt: notification_contract::CommitReceipt { sequence, revision },
            operation_id: operation_id.to_owned(),
            outcome,
            replayed: false,
        })
    }

    /// Commit events while the caller holds the state guard.
    ///
    /// A rejected batch is returned to the caller as an error. The journal
    /// owner has already rolled the batch back and poisoned this owner, so
    /// this never panics while the caller holds the process-wide state mutex.
    pub(crate) fn commit_locked(
        &self,
        st: &mut State,
        evs: &[Event],
    ) -> Result<(), notification_contract::JournalError> {
        self.commit_locked_checked(st, evs).map(|_| ())
    }

    /// Commit events while the caller holds the state guard, for callers that
    /// have no client to answer.
    ///
    /// The journal owner is already fail-closed after a rejection, so the
    /// failure is reported through the explicit failure channel and the daemon
    /// keeps serving reads instead of panicking while the state guard is held.
    /// Callers that can answer a client use `commit_locked` and propagate.
    pub(crate) fn commit_locked_reporting(&self, st: &mut State, evs: &[Event]) {
        if let Err(error) = self.commit_locked(st, evs) {
            self.report_journal_commit_error(&error);
        }
    }

    pub(crate) fn commit_locked_checked(
        &self,
        st: &mut State,
        evs: &[Event],
    ) -> Result<notification_contract::CommitReceipt, notification_contract::JournalError> {
        if let Some(error) = &st.journal_poison {
            return Err(notification_contract::JournalError::Append(error.clone()));
        }
        state::responsibility_preflight(st, evs)
            .map_err(notification_contract::JournalError::InvalidCommand)?;
        use std::io::Write;
        // Persist control truth before any state change or external notification.
        // A failed journal poisons this owner instead of silently resetting budgets.
        let mut buf = Vec::new();
        for ev in evs {
            let line = match serde_json::to_string(ev) {
                Ok(line) => line,
                Err(error) => {
                    let message = error.to_string();
                    st.journal_poison = Some(message.clone());
                    return Err(notification_contract::JournalError::Append(message));
                }
            };
            buf.extend_from_slice(line.as_bytes());
            buf.push(b'\n');
        }
        #[cfg(test)]
        let append_fault = SUBAGENT_JOURNAL_FAULT.with(|injected| {
            let injected_fault = injected.get();
            let close_final = injected_fault == SubagentJournalFault::CloseFinalAppend as u8
                && evs.iter().any(|event| {
                matches!(event, Event::SubagentUpdated { subagent } if subagent.status == "closed")
            });
            if close_final
                || injected_fault == SubagentJournalFault::StartAppend as u8
                || injected_fault == SubagentJournalFault::CloseFirstAppend as u8
                || injected_fault == SubagentJournalFault::WorkingAppend as u8
            {
                injected.set(0);
                true
            } else {
                false
            }
        });
        #[cfg(not(test))]
        let append_fault = false;
        #[cfg(feature = "context-cancel-test-hooks")]
        let peer_lifecycle_completion_fault = std::env::var(
            "COLLAB_TEST_FAIL_PEER_LIFECYCLE_COMPLETION_APPEND",
        )
        .ok()
        .is_some_and(|operation_id| {
            evs.iter().any(|event| {
                matches!(event, Event::PeerLifecycleOperationRecorded { operation }
                    if operation.operation_id == operation_id
                        && operation.phase == crate::proto::PeerLifecyclePhase::Complete)
            })
        });
        #[cfg(not(feature = "context-cancel-test-hooks"))]
        let peer_lifecycle_completion_fault = false;
        #[cfg(test)]
        let (task_register_append_fault, task_register_sync_fault) = TASK_REGISTER_JOURNAL_FAULT
            .with(|injected| {
                let injected_fault = injected.get();
                let has_task_registration = evs
                    .iter()
                    .any(|event| matches!(event, Event::TaskCreated { .. }));
                if !has_task_registration {
                    return (false, false);
                }
                if injected_fault == TaskRegisterJournalFault::Append as u8 {
                    injected.set(0);
                    (true, false)
                } else if injected_fault == TaskRegisterJournalFault::Sync as u8 {
                    injected.set(0);
                    (false, true)
                } else {
                    (false, false)
                }
            });
        #[cfg(not(test))]
        let (task_register_append_fault, task_register_sync_fault) = (false, false);
        #[cfg(test)]
        let current_thread_route_journal_fault =
            CURRENT_THREAD_ROUTE_JOURNAL_FAULT.with(|injected| {
                let has_current_thread_route = evs
                    .iter()
                    .any(|event| matches!(event, Event::GlobalCurrentThreadRouteSet { .. }));
                if has_current_thread_route {
                    injected.replace(None)
                } else {
                    None
                }
            });
        #[cfg(not(test))]
        let current_thread_route_journal_fault: Option<()> = None;
        if append_fault {
            let message = "injected subagent journal append failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        if peer_lifecycle_completion_fault {
            let message = "injected peer lifecycle completion append failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        #[cfg(test)]
        if matches!(
            current_thread_route_journal_fault,
            Some(CurrentThreadRouteJournalFault::Append)
        ) {
            let message = "injected current thread route journal append failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        if task_register_append_fault {
            let message = "injected task register journal append failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        let mut j = self.journal.lock().unwrap();
        // Record the durable length before the append so a batch the reducer
        // rejects can be rolled back instead of being replayed as an
        // unreducible event after a restart.
        let journal_len = match j.metadata() {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                let message = error.to_string();
                st.journal_poison = Some(message.clone());
                return Err(notification_contract::JournalError::Append(message));
            }
        };
        if let Err(error) = j.write_all(&buf) {
            let message = error.to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        #[cfg(test)]
        let sync_fault = SUBAGENT_JOURNAL_FAULT.with(|injected| {
            let injected_fault = injected.get();
            let close_final = injected_fault == SubagentJournalFault::CloseFinalSync as u8
                && evs.iter().any(|event| {
                matches!(event, Event::SubagentUpdated { subagent } if subagent.status == "closed")
            });
            if close_final
                || injected_fault == SubagentJournalFault::StartSync as u8
                || injected_fault == SubagentJournalFault::CloseFirstSync as u8
                || injected_fault == SubagentJournalFault::WorkingSync as u8
            {
                injected.set(0);
                true
            } else {
                false
            }
        });
        #[cfg(not(test))]
        let sync_fault = false;
        if sync_fault {
            let message = "injected subagent journal sync failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Flush(message));
        }
        #[cfg(test)]
        if matches!(
            current_thread_route_journal_fault,
            Some(CurrentThreadRouteJournalFault::Sync)
        ) {
            let message = "injected current thread route journal sync failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Flush(message));
        }
        if task_register_sync_fault {
            let message = "injected task register journal sync failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Flush(message));
        }
        if let Err(error) = j.sync_data() {
            let message = error.to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Flush(message));
        }
        if let Err(error) = self.apply_committed_events(st, evs) {
            // The append is already durable, but the reducer rejected the
            // batch. Roll the journal back to its pre-append length and re-sync
            // so a rejected event is never replayed: without this, the bad line
            // is permanent and even a restart cannot reduce the journal.
            //
            // The owner stays poisoned. `apply_committed_events` applies events
            // one by one, so the events before the rejected one are already in
            // memory and absent from the rolled-back journal. Clearing the
            // poison here would let later commits succeed on a state that no
            // longer matches the journal. Poison keeps writes fail-closed, and
            // because the poison is in-memory only, replaying the repaired
            // journal restores a clean owner.
            if let Err(rollback_error) = j.set_len(journal_len).and_then(|()| j.sync_data()) {
                st.journal_poison = Some(format!(
                    "journal rollback failed after reducer rejection: {rollback_error}"
                ));
            }
            return Err(error);
        }
        let has_pending_scheduler_admission = evs.iter().any(|event| {
            matches!(
                event,
                Event::SchedulerAdmission { admission } if admission.status == "pending"
            )
        });
        let has_succeeded_scheduler_admission = evs.iter().any(|event| {
            let Event::SchedulerAdmissionStatus {
                request_id, status, ..
            } = event
            else {
                return false;
            };
            status == "succeeded"
                && st
                    .scheduler_admissions
                    .get(request_id)
                    .is_some_and(|admission| {
                        admission.status == "succeeded"
                            && st.msgs.get(&admission.message_id).is_some_and(|message| {
                                message.state == "pending"
                                    && st.scheduler_message_deliverable(&message.id)
                            })
                    })
        });
        if (evs.iter().any(|event| matches!(event, Event::Sent { .. }))
            && !has_pending_scheduler_admission)
            || has_succeeded_scheduler_admission
            || evs
                .iter()
                .any(|event| matches!(event, Event::GlobalRuntimeBound { .. }))
        {
            self.mailbox_notify.notify_waiters();
        }
        Ok(notification_contract::CommitReceipt {
            sequence: st.sequence,
            revision: st.revision,
        })
    }

    fn append_command_phase_locked(
        &self,
        st: &mut State,
        evs: &[Event],
        phase: CommandJournalPhase,
    ) -> Result<(), notification_contract::JournalError> {
        if let Some(error) = &st.journal_poison {
            return Err(notification_contract::JournalError::Append(error.clone()));
        }
        let mut body = Vec::new();
        for ev in evs {
            let line = match serde_json::to_string(ev) {
                Ok(line) => line,
                Err(error) => {
                    let message = error.to_string();
                    st.journal_poison = Some(message.clone());
                    return Err(notification_contract::JournalError::Append(message));
                }
            };
            body.extend_from_slice(line.as_bytes());
            body.push(b'\n');
        }
        let mut journal = self.journal.lock().unwrap();
        #[cfg(test)]
        let append_fault = COMMAND_JOURNAL_FAULT.with(|injected| {
            let expected = match phase {
                CommandJournalPhase::Start => CommandJournalFault::StartAppend as u8,
                CommandJournalPhase::Completion => CommandJournalFault::CompletionAppend as u8,
                CommandJournalPhase::Business => 0,
            };
            if injected.get() == expected && expected != 0 {
                injected.set(0);
                true
            } else {
                false
            }
        });
        #[cfg(not(test))]
        let append_fault = false;
        #[cfg(not(test))]
        let _ = phase;
        if append_fault {
            let message = "injected command journal append failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        if let Err(error) = std::io::Write::write_all(&mut *journal, &body) {
            let message = error.to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Append(message));
        }
        #[cfg(test)]
        let sync_fault = COMMAND_JOURNAL_FAULT.with(|injected| {
            let expected = match phase {
                CommandJournalPhase::Start => CommandJournalFault::StartSync as u8,
                CommandJournalPhase::Completion => CommandJournalFault::CompletionSync as u8,
                CommandJournalPhase::Business => 0,
            };
            if injected.get() == expected && expected != 0 {
                injected.set(0);
                true
            } else {
                false
            }
        });
        #[cfg(not(test))]
        let sync_fault = false;
        if sync_fault {
            let message = "injected command journal sync failure".to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Flush(message));
        }
        if let Err(error) = journal.sync_data() {
            let message = error.to_string();
            st.journal_poison = Some(message.clone());
            return Err(notification_contract::JournalError::Flush(message));
        }
        Ok(())
    }

    fn apply_committed_events(
        &self,
        st: &mut State,
        evs: &[Event],
    ) -> Result<(), notification_contract::JournalError> {
        for ev in evs {
            if let Err(error) = st.apply_checked(ev) {
                st.journal_poison.get_or_insert(error.clone());
                return Err(notification_contract::JournalError::Reducer(error));
            }
            if let Err(error) = st.advance_version() {
                st.journal_poison.get_or_insert(error.clone());
                return Err(notification_contract::JournalError::Reducer(error));
            }
            if let Event::Sent { msg } = ev {
                if let Err(error) = self.backup_message(msg) {
                    self.report_mailbox_projection_error(error);
                }
            }
            if let Event::Delivered { ids } = ev {
                for id in ids {
                    if let Some(msg) = st.msgs.get(id) {
                        if let Err(error) = self.backup_message(msg) {
                            self.report_mailbox_projection_error(error);
                        }
                    }
                }
            }
            if let Event::Acked { ids } = ev {
                for id in ids {
                    if let Some(msg) = st.msgs.get(id) {
                        if let Err(error) = self.backup_message(msg) {
                            self.report_mailbox_projection_error(error);
                        }
                    }
                }
            }
        }
        if let Err(error) = st.global.validate() {
            let error = format!("global reducer validation failed: {error}");
            st.journal_poison.get_or_insert(error.clone());
            return Err(notification_contract::JournalError::Reducer(error));
        }
        Ok(())
    }

    fn report_mailbox_projection_error(&self, error: String) {
        // Journal truth is already durable. Keep projection failure explicit
        // and queryable without turning it into a false delivery result.
        append_log(
            &self.log_path(),
            &format!("MAILBOX_JSONL_WRITE_FAILED: {error}"),
        );
        if let Err(activity_error) = record_activity(
            &self.storage_root,
            "mailbox_projection_error",
            json!({"exact_error": error, "recoverable": true}),
        ) {
            append_log(
                &self.log_path(),
                &format!("MAILBOX_PROJECTION_ERROR_RECORD_FAILED: {activity_error}"),
            );
        }
    }

    /// Report a rejected reducer commit from a context that has no client to
    /// answer. The journal owner has already rolled the batch back, or set
    /// `journal_poison` when the rollback failed. This keeps the failure
    /// explicit and queryable without panicking while the caller holds the
    /// state guard.
    pub(crate) fn report_journal_commit_error(&self, error: &notification_contract::JournalError) {
        append_log(&self.log_path(), &format!("JOURNAL_COMMIT_FAILED: {error}"));
        if let Err(activity_error) = record_activity(
            &self.storage_root,
            "journal_commit_error",
            json!({"exact_error": error.to_string(), "recoverable": true}),
        ) {
            append_log(
                &self.log_path(),
                &format!("JOURNAL_COMMIT_ERROR_RECORD_FAILED: {activity_error}"),
            );
        }
    }

    fn backup_message(&self, msg: &Message) -> Result<(), String> {
        mailbox::backup_message(&self.storage_root, msg)
    }

    fn rewrite_journal_locked(
        &self,
        st: &State,
    ) -> Result<(), notification_contract::JournalError> {
        let path = self.journal_path.clone();
        let tmp = path.with_file_name("journal.jsonl.tmp");
        let mut body = String::new();
        let (sequence, revision, events) = st.snapshot_contents();
        let line = serde_json::to_string(&Event::ReducerSnapshot {
            sequence,
            revision,
            events,
        })
        .map_err(|error| {
            notification_contract::JournalError::Append(format!("compact serialize: {error}"))
        })?;
        body.push_str(&line);
        body.push('\n');
        std::fs::write(&tmp, body).map_err(|error| {
            notification_contract::JournalError::Append(format!("compact write: {error}"))
        })?;
        std::fs::rename(&tmp, &path).map_err(|error| {
            notification_contract::JournalError::Append(format!("compact rename: {error}"))
        })?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                notification_contract::JournalError::Append(format!("compact reopen: {error}"))
            })?;
        *self.journal.lock().unwrap() = file;
        Ok(())
    }
}
