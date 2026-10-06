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
        let binding_text = sanitize_identifier(&format!("binding-{worker_id}"));
        let binding_id = BindingId::new(binding_text.clone()).map_err(|error| error.to_string())?;
        let route_scope = RouteScope {
            app_scope_id: app_scope.clone(),
            project_scope_id: project_scope.clone(),
        };
        let (runtime_text, generation) = {
            let st = self.state.lock().unwrap();
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
                    let runtime_text = match (
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
                            let fingerprint =
                                bytes
                                    .into_iter()
                                    .fold(0xcbf29ce484222325_u64, |mut hash, byte| {
                                        hash ^= u64::from(byte);
                                        hash.wrapping_mul(0x100000001b3)
                                    });
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
            }
        };
        let (registration, registered_ms) = {
            let st = self.state.lock().unwrap();
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
            (registration, registered_ms)
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
        let expected_revision = {
            let st = self.state.lock().unwrap();
            st.revision
        };
        let base_command = format!("register-{binding_text}-{generation}");
        let prior_attempt = !reuse_existing && self.state.lock().unwrap().global
            .lookup_command_receipt(&CommandId::new(base_command.clone()).map_err(|error| error.to_string())?)
            .is_some();
        let attempt_suffix = prior_attempt.then(|| format!("-retry-{expected_revision}"))
            .unwrap_or_default();
        let command_id = CommandId::new(format!("{base_command}{attempt_suffix}"))
            .map_err(|error| error.to_string())?;
        let operation_id = OperationId::new(format!(
            "register-op-{binding_text}-{generation}{attempt_suffix}"
        ))
        .map_err(|error| error.to_string())?;
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
        Ok(TypedEnvelope {
            command: TypedCommand::RegisterWorker {
                registration,
                binding,
                worker,
            },
            envelope,
        })
    }

    /// Dispatch one typed command through validation, journal append, flush
    /// and reducer apply. This is the production typed seam above the legacy
    /// CLI adapters; the legacy v1 call site routes through it.
    pub fn typed_dispatch(
        &self,
        typed: TypedEnvelope,
    ) -> Result<state::TypedOutcome, notification_contract::JournalError> {
        typed.envelope.validate().map_err(|error| {
            notification_contract::JournalError::InvalidCommand(error.to_string())
        })?;
        let mut st = self.state.lock().unwrap();
        self.validate_typed_register(&st, &typed)?;
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
                                    crate::client::adapters::tmux::same_pane_route(old, new)
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
                if !same_route || !same_runtime_thread {
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
        self.commit_command_locked(&mut st, command_id, operation_id, evs, outcome, None)
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
    pub(crate) fn report_journal_commit_error(
        &self,
        error: &notification_contract::JournalError,
    ) {
        append_log(
            &self.log_path(),
            &format!("JOURNAL_COMMIT_FAILED: {error}"),
        );
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
        let events = st.snapshot_events();
        for (index, event) in events.iter().enumerate() {
            let line = serde_json::to_string(event).map_err(|error| {
                notification_contract::JournalError::Append(format!("compact serialize: {error}"))
            })?;
            body.push_str(&line);
            let next_is_checkpoint = events
                .get(index + 1)
                .is_some_and(|next| matches!(next, Event::ReducerCheckpoint { .. }));
            if index + 1 != events.len() && !next_is_checkpoint {
                body.push('\n');
            }
        }
        if events
            .last()
            .is_some_and(|event| matches!(event, Event::ReducerCheckpoint { .. }))
        {
            body.push('\n');
        }
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

fn validate_transport_candidates(
    server: &Server,
    candidates: &TransportCandidates,
    registration_cwd: &str,
) -> Result<SelectedTransport, String> {
    let requested_root = std::fs::canonicalize(registration_cwd)
        .map_err(|error| format!("RUNTIME_BINDING_REJECTED: registration cwd: {error}"))?;
    let candidate_root = std::fs::canonicalize(&server.root)
        .map_err(|error| format!("RUNTIME_BINDING_REJECTED: project root: {error}"))?;
    if candidate_root != requested_root {
        return Err(format!(
            "RUNTIME_BINDING_REJECTED: registration cwd {} does not match project root {}",
            requested_root.display(),
            candidate_root.display()
        ));
    }
    // dsh is a mutually exclusive channel: it has no pane to act as an App
    // Server recovery anchor, and a caller supplying dsh *and* a tmux/appserver
    // candidate has ambiguous intent. Resolving that silently is exactly the
    // candidate-shadowing this design forbids, so the ambiguous set is refused
    // rather than ranked.
    if candidates.dsh.is_some() && (candidates.appserver.is_some() || candidates.tmux.is_some()) {
        return Err(
            "DSH_ENDPOINT_REJECTED: a dsh candidate must not be combined with an App Server or tmux candidate"
                .into(),
        );
    }
    if let Some(dsh) = candidates.dsh.as_ref() {
        return admit_dsh_candidate(dsh, &candidate_root);
    }
    if let Some(appserver) = candidates.appserver.as_ref() {
        let app_cwd = std::fs::canonicalize(&appserver.cwd).map_err(|error| {
            format!("RUNTIME_BINDING_REJECTED: App Server candidate cwd: {error}")
        })?;
        if app_cwd != candidate_root {
            return Err(format!(
                "RUNTIME_BINDING_REJECTED: App Server candidate cwd {} does not match project root {}",
                app_cwd.display(),
                candidate_root.display()
            ));
        }
        let mut selected = crate::client::adapters::verify_candidate(appserver)
            .map_err(|error| format!("APPSERVER_ENDPOINT_REJECTED: {error}"))?;
        if let Some(tmux) = candidates.tmux.as_ref() {
            if !tmux.cwd.starts_with('/') {
                return Err("RUNTIME_BINDING_REJECTED: tmux candidate cwd must be absolute".into());
            }
            match crate::client::adapters::tmux::probe(&tmux.endpoint)? {
                crate::client::adapters::tmux::PanePresence::Present => {}
                crate::client::adapters::tmux::PanePresence::Missing => {
                    return Err("TMUX_PANE_MISSING: recovery pane is not live".into())
                }
                crate::client::adapters::tmux::PanePresence::Unknown => {
                    return Err("TMUX_PANE_UNKNOWN: recovery pane liveness is uncertain".into())
                }
            }
            let mut recovery = tmux.endpoint.clone();
            if recovery.codex_session_id.is_none() {
                recovery.codex_session_id = Some(appserver.session_id.clone());
            }
            if recovery.codex_thread_id.is_none() {
                recovery.codex_thread_id = Some(appserver.thread_id.clone());
            }
            selected.tmux_endpoint = Some(recovery);
            if !selected
                .capabilities
                .iter()
                .any(|cap| cap == "pane_recovery_anchor")
            {
                selected.capabilities.push("pane_recovery_anchor".into());
            }
        }
        return Ok(selected);
    }
    if let Some(candidate) = candidates.tmux.as_ref() {
        if !candidate.cwd.starts_with('/') {
            return Err("RUNTIME_BINDING_REJECTED: tmux candidate cwd must be absolute".into());
        }
        match crate::client::adapters::tmux::probe(&candidate.endpoint)? {
            crate::client::adapters::tmux::PanePresence::Present => {}
            crate::client::adapters::tmux::PanePresence::Missing => {
                return Err("TMUX_PANE_MISSING: registration pane is not live".into())
            }
            crate::client::adapters::tmux::PanePresence::Unknown => {
                return Err("TMUX_PANE_UNKNOWN: registration pane liveness is uncertain".into())
            }
        }
        if candidate
            .endpoint
            .codex_session_id
            .as_deref()
            .is_some_and(|id| id.trim().is_empty())
            || candidate
                .endpoint
                .codex_thread_id
                .as_deref()
                .is_some_and(|id| id.trim().is_empty())
        {
            return Err("TMUX_IDENTITY_INVALID: empty Codex identity anchor".into());
        }
        return Ok(SelectedTransport {
            kind: TransportKind::Tmux,
            endpoint: Some(candidate.endpoint.socket_path.clone()),
            namespace: Some(candidate.endpoint.tmux_session_id.clone()),
            session_id: candidate
                .endpoint
                .codex_session_id
                .clone()
                .or_else(|| Some(candidate.endpoint.tmux_session_id.clone())),
            thread_id: candidate
                .endpoint
                .codex_thread_id
                .clone()
                .or_else(|| Some(candidate.endpoint.pane_id.clone())),
            tmux_endpoint: Some(candidate.endpoint.clone()),
            capabilities: vec!["send_message_to_pane".into(), "probe_pane".into()],
            self_check: "tmux socket, session, pane and pane pid verified".into(),
        });
    }
    Err("TRANSPORT_NONE: no reachable App Server, tmux or dsh candidate was supplied".into())
}

/// Admits a dsh candidate by challenging the gateway control socket once.
///
/// Every field must agree in the *same* response: the nonce proves the reply
/// belongs to this challenge, and runtime/agent/session/cwd are compared
/// field-by-field. Any mismatch is a rejection, never a degraded admission.
fn admit_dsh_candidate(
    candidate: &crate::proto::DshCandidate,
    candidate_root: &Path,
) -> Result<SelectedTransport, String> {
    if !candidate.cwd.starts_with('/') {
        return Err("DSH_ENDPOINT_REJECTED: dsh candidate cwd must be absolute".into());
    }
    let dsh_cwd = std::fs::canonicalize(&candidate.cwd)
        .map_err(|error| format!("DSH_ENDPOINT_REJECTED: dsh candidate cwd: {error}"))?;
    if dsh_cwd != candidate_root {
        return Err(format!(
            "DSH_ENDPOINT_REJECTED: dsh candidate cwd {} does not match project root {}",
            dsh_cwd.display(),
            candidate_root.display()
        ));
    }
    let facts = crate::client::adapters::dsh::facts(
        &candidate.endpoint,
        &candidate.runtime_id,
        &candidate.agent_id,
    )
    .map_err(|error| error.to_string())?;
    if facts.runtime_id != candidate.runtime_id {
        return Err(format!(
            "DSH_ENDPOINT_REJECTED: gateway reports runtime {} for candidate runtime {}",
            facts.runtime_id, candidate.runtime_id
        ));
    }
    if facts.agent_id != candidate.agent_id {
        return Err(format!(
            "DSH_ENDPOINT_REJECTED: gateway reports agent {} for candidate agent {}",
            facts.agent_id, candidate.agent_id
        ));
    }
    if facts.session_id != candidate.session_id {
        return Err(format!(
            "DSH_ENDPOINT_REJECTED: gateway reports session {} for candidate session {}",
            facts.session_id, candidate.session_id
        ));
    }
    if facts.status.trim().is_empty() {
        return Err("DSH_ENDPOINT_REJECTED: gateway reported an empty agent status".into());
    }
    let reported_cwd = std::fs::canonicalize(&facts.cwd).map_err(|error| {
        format!(
            "DSH_ENDPOINT_REJECTED: gateway reported dsh cwd {}: {error}",
            facts.cwd
        )
    })?;
    if reported_cwd != *candidate_root {
        return Err(format!(
            "DSH_ENDPOINT_REJECTED: gateway reports dsh cwd {} outside project root {}",
            reported_cwd.display(),
            candidate_root.display()
        ));
    }
    Ok(SelectedTransport {
        kind: TransportKind::Dsh,
        endpoint: Some(candidate.endpoint.clone()),
        namespace: Some(candidate.runtime_id.clone()),
        session_id: Some(candidate.session_id.clone()),
        thread_id: Some(candidate.agent_id.clone()),
        tmux_endpoint: None,
        capabilities: vec!["enqueue_wake".into(), "agent_facts".into()],
        self_check: format!(
            "gateway control socket answered a single-use nonce challenge; runtime, agent, session and cwd verified; reported status {}",
            facts.status
        ),
    })
}

type RouteKey = (String, String);

/// A host route is only a small admission record.  The reducer and mailbox
/// facts belong to the runtime selected by this record, never to the
/// resident project's journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostRouteRecord {
    pub(crate) version: u8,
    pub(crate) op: String,
    pub(crate) app_scope_id: String,
    pub(crate) project_scope: String,
    pub(crate) canonical_root: String,
    pub(crate) storage_root: String,
    pub(crate) registered_ms: i64,
}

pub(crate) const ROUTE_RESOLVE_NOT_FOUND_RECOVERY: &str = "recovery: run `collab context` from the canonical project main checkout; it resolves the canonical root, restores identity and registration, starts the daemon when no explicit DOWN marker exists, and returns the current role's operations. Do not re-register a worktree, edit routes.jsonl, copy identity tokens, start a second daemon, or use mailbox state as transport delivery";

struct RuntimeRoute {
    root: PathBuf,
    storage_root: PathBuf,
    runtime: Option<Arc<Server>>,
}

/// Owns the single host listener's route table and the independent project
/// reducers behind it.  The existing handler surface remains unchanged: a
/// request is first routed here, then dispatched to the selected `Server`.
struct ProjectRuntimeManager {
    host: Arc<Server>,
    host_root: PathBuf,
    route_journal: PathBuf,
    routes: Mutex<std::collections::BTreeMap<RouteKey, RuntimeRoute>>,
    project_locks: Mutex<std::collections::BTreeMap<PathBuf, std::fs::File>>,
    register_gate: Mutex<()>,
    runtime_init_gates: Mutex<std::collections::BTreeMap<RouteKey, Arc<Mutex<()>>>>,
    #[cfg(test)]
    fail_current_thread_route_publish: std::sync::atomic::AtomicBool,
}

fn storage_owner_path(path: &Path) -> Result<PathBuf, String> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut missing_suffix = Vec::new();
            let mut current = path.to_path_buf();
            loop {
                match std::fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        let target = std::fs::read_link(&current).map_err(|error| {
                            format!("storage owner symlink {}: {error}", current.display())
                        })?;
                        let target = if target.is_absolute() {
                            target
                        } else {
                            current
                                .parent()
                                .filter(|parent| !parent.as_os_str().is_empty())
                                .unwrap_or_else(|| Path::new("."))
                                .join(target)
                        };
                        let mut resolved = storage_owner_path(&target)?;
                        for component in missing_suffix.iter().rev() {
                            resolved.push(component);
                        }
                        return Ok(resolved);
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("storage owner path {}: {error}", current.display()));
                    }
                }
                let Some(name) = current.file_name() else {
                    return Err(format!(
                        "storage owner path has no resolvable ancestor: {}",
                        path.display()
                    ));
                };
                missing_suffix.push(name.to_os_string());
                let parent = current
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                match std::fs::canonicalize(parent) {
                    Ok(mut resolved) => {
                        for component in missing_suffix.iter().rev() {
                            resolved.push(component);
                        }
                        return Ok(resolved);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        current = parent.to_path_buf();
                    }
                    Err(error) => {
                        return Err(format!(
                            "storage owner path ancestor {}: {error}",
                            parent.display()
                        ));
                    }
                }
            }
        }
        Err(error) => Err(format!("storage owner path {}: {error}", path.display())),
    }
}

fn storage_roots_equal(left: &Path, right: &Path) -> Result<bool, String> {
    Ok(storage_owner_path(left)? == storage_owner_path(right)?)
}

fn is_resident_self_route(
    app_scope_id: &str,
    project_scope: &str,
    canonical_root: &Path,
    storage_root: &Path,
    host_root: &Path,
    host_storage_root: &Path,
    host: &Arc<Server>,
    existing_routes: &std::collections::BTreeMap<RouteKey, RuntimeRoute>,
) -> Result<bool, String> {
    if canonical_root != host_root || project_scope != host_root.to_string_lossy() {
        return Ok(false);
    }
    let storage_is_host_owned = storage_roots_equal(storage_root, host_root)?
        || storage_roots_equal(storage_root, host_storage_root)?;
    if !storage_is_host_owned {
        return Ok(false);
    }
    match existing_routes.get(&(app_scope_id.to_owned(), project_scope.to_owned())) {
        Some(route) => Ok(route
            .runtime
            .as_ref()
            .is_some_and(|runtime| Arc::ptr_eq(runtime, host))),
        // The durable route record is the recovery evidence when the
        // registration did not survive replay. Canonical root plus a
        // host-owned storage root cannot be an ordinary non-resident route.
        None => Ok(true),
    }
}

fn sync_parent_dir(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path has no parent directory",
        )
    })?;
    std::fs::File::open(parent)?.sync_all()
}

include!("append_host_route_record.rs");

fn validate_runtime_storage_root(
    root: &Path,
    storage_root: &Path,
    error_prefix: &str,
) -> Result<PathBuf, String> {
    if !storage_root.is_absolute() {
        return Err(format!(
            "{error_prefix}: runtime storage root must be absolute"
        ));
    }
    let root_owner = storage_owner_path(root)
        .map_err(|error| format!("{error_prefix}: resolve project storage owner: {error}"))?;
    let storage_owner = storage_owner_path(storage_root)
        .map_err(|error| format!("{error_prefix}: resolve runtime storage owner: {error}"))?;
    if storage_owner == root_owner {
        return Ok(root_owner);
    }
    let expected_parent = root_owner
        .join(".agent-collab")
        .join("server")
        .join("runtimes");
    if !storage_owner.starts_with(&expected_parent) {
        return Err(format!(
            "{error_prefix}: runtime storage root {} resolves outside project runtime storage {}",
            storage_root.display(),
            expected_parent.display()
        ));
    }
    Ok(storage_owner)
}
