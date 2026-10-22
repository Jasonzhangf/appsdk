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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    continuation: Option<register_reply::Continuation>,
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
            continuation: None,
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
            continuation: None,
        }
    }

    /// The approved-recovery proof for this operation, if any. Ordinary
    /// automatic/supplement operations never carry one.
    pub(crate) fn approval(&self) -> Option<&RegisterApprovalProof> {
        self.approval.as_deref()
    }

    pub(crate) fn with_continuation(
        mut self,
        context: ProjectContext,
        request: IdentityContextRequest,
    ) -> Self {
        self.continuation = Some(register_reply::Continuation { context, request });
        self
    }

    /// Append and sync the outer `Validating` phase with the exact nested IDs
    /// of `prepared`. On any failure the bound state stays `None` and durable
    /// state is unchanged, so the caller must not consume.
    pub(crate) fn bind_validating(
        &self,
        prepared: &PreparedRegisterEnvelope,
    ) -> Result<(), String> {
        self.bind_validating_ids(&prepared.nested_command_id, &prepared.nested_operation_id)
    }

    /// Bind a proven committed receipt without preparing or replaying Register.
    pub(crate) fn bind_committed_receipt(
        &self,
        receipt: &global_state::CommandReceipt,
    ) -> Result<(), String> {
        self.bind_validating_ids(receipt.command_id.as_str(), receipt.operation_id.as_str())
    }

    fn bind_validating_ids(&self, command_id: &str, operation_id: &str) -> Result<(), String> {
        if let Some(continuation) = &self.continuation {
            let existing = self
                .journal
                .query(
                    &continuation.request,
                    continuation.context.project_scope.as_str(),
                    continuation.context.app_scope_id.as_str(),
                )?
                .result;
            if existing.phase == crate::proto::IdentityOperationPhase::Validating
                && existing.nested_command_id.as_deref() == Some(command_id)
                && existing.nested_operation_id.as_deref() == Some(operation_id)
            {
                // Participant continuation restores the original typed
                // binding. It must not append Validating a second time.
                *self.bound.lock().unwrap() =
                    Some((command_id.to_owned(), operation_id.to_owned()));
                return Ok(());
            }
        }
        self.journal
            .transition_with_business_receipts(
                &self.operation_id,
                crate::proto::IdentityOperationPhase::Validating,
                Some(command_id.to_owned()),
                Some(operation_id.to_owned()),
                Some(self.initial_business_receipts.clone()),
            )
            .map(|_| {
                *self.bound.lock().unwrap() =
                    Some((command_id.to_owned(), operation_id.to_owned()));
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

/// The sole material algorithm for newly prepared Register identifiers.
fn scoped_register_command_prefix(
    scope: &RouteScope,
    binding: &BindingId,
    generation: u64,
) -> String {
    use sha2::{Digest, Sha256};
    let material = serde_json::to_vec(&(
        "collab-register-scope/v1",
        &scope.project_scope_id,
        &scope.app_scope_id,
        binding,
        generation,
    ))
    .expect("typed Register ID tuple contains only scalar values");
    format!("register-{:x}", Sha256::digest(material))
}

/// Exact committed-name compatibility, not a lookup fallback or new key owner.
fn register_receipt_names_binding(
    receipt: &global_state::CommandReceipt,
    binding: &RuntimeBinding,
) -> bool {
    let scoped = scoped_register_command_prefix(
        &binding.route_scope(),
        &binding.binding_id,
        binding.endpoint_generation,
    );
    let legacy = format!(
        "register-{}-{}",
        binding.binding_id, binding.endpoint_generation
    );
    [scoped, legacy].iter().any(|prefix| {
        let Some(suffix) = receipt.command_id.as_str().strip_prefix(prefix) else {
            return false;
        };
        let valid_suffix = suffix.is_empty()
            || suffix.strip_prefix("-retry-").is_some_and(|revision| {
                !revision.is_empty() && revision.bytes().all(|byte| byte.is_ascii_digit())
            });
        valid_suffix
            && receipt.operation_id.as_str()
                == format!(
                    "register-op-{}{suffix}",
                    prefix.strip_prefix("register-").unwrap()
                )
    })
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
        // Derive IDs and the binding from one snapshot. Registration is
        // fenced by its principal and endpoint generation, not by unrelated
        // ledger or timer revisions. Explicit caller CAS remains supported.
        let (
            runtime_text,
            generation,
            registration,
            registered_ms,
            snapshot_revision,
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
                                    format!("runtime-tmux-{}", sanitize_identifier(worker_id))
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
            let base_command =
                scoped_register_command_prefix(&route_scope, &binding_id, generation);
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
            generation,
            session_id,
            native_thread_id,
        )
        .map_err(|error| error.to_string())?;
        binding.tmux_endpoint = transport.tmux_endpoint.clone();
        binding.validate().map_err(|error| error.to_string())?;
        let attempt_suffix = prior_attempt
            .then(|| format!("-retry-{snapshot_revision}"))
            .unwrap_or_default();
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
            generation,
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
        self.typed_dispatch_register(typed, approval, outer_binding)
    }

    fn typed_dispatch_register(
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
                    if let Some(endpoint) = binding.tmux_endpoint.as_ref() {
                        for claimant in pane_claimants(
                            &st,
                            binding.agent_id.as_str(),
                            &binding.route_scope(),
                            endpoint,
                        ) {
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
                        &st,
                        binding.agent_id.as_str(),
                        &binding.binding_id,
                        &binding.route_scope(),
                        &binding,
                    ) {
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
                            .map_err(notification_contract::JournalError::InvalidCommand)?,
                        );
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
}
include!("part_02/commit.rs");

#[cfg(test)]
mod scoped_register_tests {
    use super::*;

    fn prepared(server: &Server, root: &Path, worker: &str) -> PreparedRegisterEnvelope {
        let transport = SelectedTransport {
            kind: TransportKind::AppServer,
            endpoint: Some("unix:///tmp/scoped-register.sock".into()),
            namespace: Some("codex_tui".into()),
            session_id: Some(format!("session-{worker}")),
            thread_id: Some(format!("thread-{worker}")),
            tmux_endpoint: None,
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
        let TypedCommand::RegisterWorker { binding, .. } = &first.typed.command;
        let binding = binding.clone();
        let mut legacy = first.typed.clone();
        legacy.envelope.command_id = CommandId::new(format!(
            "register-{}-{}",
            binding.binding_id, binding.endpoint_generation
        ))
        .unwrap();
        legacy.envelope.operation_id = OperationId::new(format!(
            "register-op-{}-{}",
            binding.binding_id, binding.endpoint_generation
        ))
        .unwrap();
        let old_ids = (
            legacy.envelope.command_id.clone(),
            legacy.envelope.operation_id.clone(),
        );
        let original = server.typed_dispatch(legacy.clone()).unwrap();
        assert!(register_receipt_names_binding(&original.receipt, &binding));
        let before = std::fs::read(&server.journal_path).unwrap();
        let replay = server.typed_dispatch(legacy).unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.receipt.command_id, old_ids.0);
        assert_eq!(replay.receipt.operation_id, old_ids.1);
        assert_eq!(std::fs::read(&server.journal_path).unwrap(), before);
        let mut mismatched = original.receipt;
        mismatched.operation_id = OperationId::new("unrelated-operation").unwrap();
        assert!(!register_receipt_names_binding(&mismatched, &binding));
        std::fs::remove_dir_all(root).unwrap();
    }

}
