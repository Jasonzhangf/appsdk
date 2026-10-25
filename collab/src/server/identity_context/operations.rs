impl ProjectRuntimeManager {
    pub(super) fn identity_context(
        &self,
        context: ProjectContext,
        request: IdentityContextRequest,
    ) -> (Arc<Server>, Resp) {
        self.identity_context_continue(context, request, false, None)
    }

    fn identity_context_continue(
        &self,
        context: ProjectContext,
        request: IdentityContextRequest,
        continuing: bool,
        continuation_facts: Option<IdentityFacts>,
    ) -> (Arc<Server>, Resp) {
        let bootstrap_policy = request
            .grant_approval
            .as_ref()
            .is_some_and(|raw| raw["action"] == "authorize_first_registered_peer");
        if request.action == "prepare_invocation" {
            return self.prepare_context_invocation(&context, request);
        }
        if request.action == "cancel" {
            return self.cancel_context_operation(&context, request);
        }
        if !request.query && !bootstrap_policy {
            if let Err(error) = self.reconcile_current_dsh_session(&context, &request.facts) {
                return (self.host.clone(), Resp::err(error.to_string()));
            }
        }
        let _guard = self.identity_gate.lock().unwrap();
        if request.operation_id.is_empty() {
            // A bootstrap policy commit is a durable global owner and carries
            // the caller's operation id. Without one, the legacy branch below
            // would run ordinary registration and silently ignore the policy.
            // Reject before any owner effect; never synthesize missing identity.
            if bootstrap_policy {
                return (
                    self.host.clone(),
                    Resp::err(
                        "IDENTITY_OPERATION_ID_REQUIRED: bootstrap policy authorization requires a durable operation id",
                    ),
                );
            }
            match name_adjudication_control(&request) {
                Ok(Some(_)) => {
                    if let Err(ApprovedAdmissionError::Error(error)) =
                        self.approved_recovery_admission(&context, &request)
                    {
                        return (self.host.clone(), Resp::err(error));
                    }
                }
                Err(ApprovedAdmissionError::Error(error)) => {
                    return (self.host.clone(), Resp::err(error))
                }
                Ok(None) => {}
            }
            return match self.reconcile_identity_context(
                context.clone(),
                request.facts.clone(),
                None,
                None,
                true,
            ) {
                Ok((runtime, mut response, _)) => {
                    if response.ok && !response.data["identity_receipt"].is_null() {
                        if let Err(error) =
                            self.resolve_context_name(&runtime, &context, &request, &mut response)
                        {
                            return (runtime, Resp::err_data(error, response.data));
                        }
                    }
                    (runtime, response)
                }
                Err(error) => (self.host.clone(), Resp::err(format!("{error:#}"))),
            };
        }
        if let Err(error) = validate_operation_request(&request) {
            return (self.host.clone(), Resp::err(error));
        }
        // Same-key retry: a durable operation for this key and capability
        // already exists. Re-admit only to detect a changed normalized intent,
        // then return the retained projection without repeating any side
        // effect or reconciliation. Check this before consuming the one-shot
        // invocation ticket: a completed operation necessarily consumed it.
        if let Ok(existing) = self.operation_journal.query(
            &request,
            context.project_scope.as_str(),
            context.app_scope_id.as_str(),
        ) {
            if !continuing || existing.result.outcome == "completed" {
                return match self.admit_identity_operation(&context, &request) {
                    Ok(_) => {
                        let mut projection = existing.result;
                        if let Some(readback) = self
                            .name_recovery_readback(&context, &request.operation_id)
                            .filter(|readback| {
                                projection.phase
                                    == crate::proto::IdentityOperationPhase::EffectObserved
                                    && readback["outcome"] == "applied"
                            })
                        {
                            let mut receipts = projection.business_receipts.clone();
                            if receipts.iter().any(|receipt| receipt == "lease") {
                                let effects: name_resolution::NameResolutionEffects =
                                    match serde_json::from_value(readback["effects"].clone()) {
                                        Ok(effects) => effects,
                                        Err(error) => {
                                            return (
                                                self.host.clone(),
                                                Resp::err(format!("NAME_EFFECTS_INVALID: {error}")),
                                            )
                                        }
                                    };
                                if effects.grant_committed
                                    && !receipts.iter().any(|receipt| receipt == "grant")
                                {
                                    let position = receipts
                                        .iter()
                                        .position(|receipt| receipt == "lease")
                                        .unwrap();
                                    receipts.insert(position, "grant".into());
                                }
                                receipts.push("context_complete".into());
                                projection = match self.finish_identity_operation(
                                    &context, &request, None, true, receipts,
                                ) {
                                    Ok(projection) => projection,
                                    Err(error) => return (self.host.clone(), Resp::err(error)),
                                };
                            }
                        }
                        let completed = projection.outcome == "completed";
                        let (runtime, snapshot) = if completed
                            && request.action == "context"
                            && !bootstrap_policy
                        {
                            match self.completed_context_readback(
                                &context,
                                continuation_facts.as_ref().unwrap_or(&request.facts),
                            ) {
                                Ok((runtime, snapshot)) => (runtime, Some(snapshot)),
                                Err(error) => {
                                    return (
                                        self.host.clone(),
                                        Resp::err(format!(
                                            "IDENTITY_CONTEXT_READBACK_FAILED: {error:#}"
                                        )),
                                    )
                                }
                            }
                        } else {
                            (self.host.clone(), None)
                        };
                        let result = operation_result(&request, projection, completed, snapshot);
                        (runtime, typed_operation_resp(result))
                    }
                    Err(error) if error.starts_with("IDENTITY_OPERATION_INTENT_CONFLICT") => (
                        self.host.clone(),
                        typed_operation_resp(operation_refusal(
                            &request,
                            error,
                            "collab context --op <new-operation-id>",
                        )),
                    ),
                    Err(error) => (self.host.clone(), Resp::err(error)),
                };
            }
        }
        let digest = identity_operation_intent_digest(&context, &request);
        if let Err(error) = if continuing {
            Ok(())
        } else {
            self.operation_journal.claim_invocation(
                &request.operation_id,
                &request.invocation_ticket,
                &digest,
                &crate::server::operation_journal::capability_hash(&request.query_capability),
            )
        } {
            if error == "IDENTITY_OPERATION_CANCELLED" {
                let cancelled = operation_result(
                    &request,
                    crate::proto::IdentityOperationProjection {
                        operation_id: request.operation_id.clone(),
                        phase: crate::proto::IdentityOperationPhase::Cancelled,
                        outcome: "cancelled".into(),
                        committed_phases: Vec::new(),
                        business_receipts: Vec::new(),
                        nested_command_id: None,
                        nested_operation_id: None,
                    },
                    false,
                    None,
                );
                return (self.host.clone(), typed_operation_resp(cancelled));
            }
            return (self.host.clone(), Resp::err(error));
        }
        if bootstrap_policy {
            return self.authorize_bootstrap_policy(&context, &request);
        }
        // New approved recovery requires both owner fences before durable
        // admission. A same-key retry above returns its retained projection;
        // it must not re-evaluate an incumbent already changed by that commit.
        let approved = match self.approved_recovery_admission(&context, &request) {
            Ok(approved) => approved,
            Err(ApprovedAdmissionError::Error(error)) => {
                return (self.host.clone(), Resp::err(error));
            }
        };
        let facts = match self.preflight_identity_facts(
            &context,
            continuation_facts.unwrap_or_else(|| request.facts.clone()),
        ) {
            Ok(facts) => facts,
            Err(PreflightIdentityError::MissingFacts(data)) => {
                // missing_facts is reported before admission: no operation
                // record exists and no side effect ran. Release the consumed
                // preflight ticket so the agent can submit the requested facts
                // under this same operation key.
                if !request.invocation_ticket.is_empty() {
                    if let Err(error) = self.operation_journal.release_missing_facts_invocation(
                        &request.operation_id,
                        &request.invocation_ticket,
                    ) {
                        return (self.host.clone(), Resp::err(error));
                    }
                }
                let result = missing_facts_result(&request, &data);
                return (self.host.clone(), typed_operation_resp(result));
            }
            Err(PreflightIdentityError::Error(error)) => {
                return (self.host.clone(), Resp::err(format!("{error:#}")));
            }
        };
        let admitted = match self.admit_identity_operation(&context, &request) {
            Ok(operation) => operation,
            Err(error) => return (self.host.clone(), Resp::err(error)),
        };
        #[cfg(feature = "context-cancel-test-hooks")]
        crate::context_cancel_test_hooks::barrier(
            "admitted_before_owner",
            &request.operation_id,
            std::process::id(),
            None,
            None,
            || false,
        );
        // Admitted-before-first-owner safe boundary: if a cancellation won
        // while we were paused here, return the durable cancelled projection
        // and never start the owner.
        if self.operation_journal.is_cancelled(&request.operation_id) {
            let projection = self
                .latest_projection(&context, &request)
                .unwrap_or(admitted.clone());
            let result = operation_result(&request, projection, false, None);
            return (self.host.clone(), typed_operation_resp(result));
        }
        let outer_binding = match approved.as_ref() {
            Some(approved) => RegisterOuterBinding::with_approval(
                self.operation_journal.clone(),
                request.operation_id.clone(),
                approved.register_proof(&context),
            ),
            None => RegisterOuterBinding::new(
                self.operation_journal.clone(),
                request.operation_id.clone(),
            ),
        }
        .with_continuation(context.clone(), request.clone());
        let approved_identity = approved.as_ref().map(|approved| approved.identity.clone());
        // Do not hold identity serialization across selected-path dispatch.
        drop(_guard);
        let reconciliation = self.reconcile_identity_context(
            context.clone(),
            facts,
            Some(outer_binding.clone()),
            approved_identity,
            approved.as_ref().is_none_or(|approved| {
                matches!(
                    approved.identity_approval.action.as_str(),
                    "restore_master" | "restore_peer"
                )
            }),
        );
        let reconciliation = if request.invocation == "manual_communication_restore" {
            self.manual_communication_gate(&context, &request, &outer_binding, reconciliation)
        } else {
            reconciliation
        };
        let _guard = self.identity_gate.lock().unwrap();
        match reconciliation {
            Ok((runtime, mut response, nested)) if response.ok => {
                let bound = outer_binding.bound();
                let nested_ref = nested.as_ref().or(bound.as_ref());
                // Business receipts are projected only from durable owner
                // evidence, in the accepted public order: approval decision,
                // nested Register, route, credential, optional grant, lease,
                // then the terminal `context_complete`. The helper stops at the
                // first owner whose durable proof is missing.
                let mut business_receipts = business_receipts_for_success(
                    &self.host,
                    &runtime,
                    &context,
                    approved.as_ref(),
                    &response,
                    nested_ref,
                );
                // Between-owners safe boundary: the Register owner has
                // committed its proven work. Expose exactly those receipts so
                // cancellation can win before the next owner starts.
                self.operation_journal
                    .reopen_safe(&request.operation_id, business_receipts.clone());
                #[cfg(feature = "context-cancel-test-hooks")]
                crate::context_cancel_test_hooks::barrier(
                    "between_owners",
                    &request.operation_id,
                    std::process::id(),
                    nested_ref.map(|(command_id, _)| command_id.as_str()),
                    nested_ref.map(|(_, operation_id)| operation_id.as_str()),
                    || false,
                );
                if self.operation_journal.is_cancelled(&request.operation_id) {
                    let projection = self
                        .latest_projection(&context, &request)
                        .unwrap_or(admitted.clone());
                    let result = operation_result(
                        &request,
                        projection,
                        false,
                        response.data.get("snapshot").cloned(),
                    );
                    return (runtime, typed_operation_resp(result));
                }
                let mut grant_committed = false;
                if !response.data["identity_receipt"].is_null() {
                    let resolution = self
                        .operation_journal
                        .begin_owner_start(&request.operation_id, "name")
                        .and_then(|_| {
                            self.converge_context_session_carriers(&runtime, &context, &mut response)?;
                            self.resolve_context_name(&runtime, &context, &request, &mut response)
                        });
                    if resolution.is_err()
                        || !matches!(
                            response.data["snapshot"]["name_resolution"]["outcome"].as_str(),
                            Some("committed" | "applied")
                        )
                    {
                        let operation = if resolution.is_ok() {
                            self.record_name_pending(
                                &context,
                                &request,
                                admitted,
                                business_receipts,
                            )
                        } else {
                            self.record_incomplete_operation(
                                &context,
                                &request,
                                admitted,
                                nested.or_else(|| outer_binding.bound()),
                                outer_binding.bound().is_some(),
                                business_receipts,
                            )
                        };
                        let mut result = operation_result(
                            &request,
                            operation,
                            false,
                            response.data.get("snapshot").cloned(),
                        );
                        if let Err(error) = resolution {
                            result
                                .owner_readback
                                .insert("name_resolution".into(), json!({"error":error}));
                        } else {
                            result.requires.kind = response.data["snapshot"]["name_resolution"]
                                ["outcome"]
                                .as_str()
                                .map(str::to_owned);
                        }
                        return (runtime, typed_operation_resp(result));
                    }
                    grant_committed = resolution
                        .expect("successful name resolution")
                        .grant_committed;
                }
                if let Some(approved) = approved.as_ref() {
                    // The distinct grant owner commits and reads back its own
                    // effect only when a separate grant approval was supplied.
                    // Identity-only recovery keeps the existing authority owner
                    // and never promotes, clears, or replaces it.
                    if approved.requires_grant_replacement() {
                        self.operation_journal
                            .reopen_safe(&request.operation_id, business_receipts.clone());
                        if let Err(reason) = self.replace_approved_master_grant(
                            &runtime,
                            &context,
                            &request,
                            &mut response,
                            approved,
                        ) {
                            // A cancellation that won at the between-owners
                            // boundary must be returned as the durable
                            // cancelled projection, never as a repair result.
                            if self.operation_journal.is_cancelled(&request.operation_id) {
                                let projection = self
                                    .latest_projection(&context, &request)
                                    .unwrap_or(admitted.clone());
                                let result = operation_result(
                                    &request,
                                    projection,
                                    false,
                                    response.data.get("snapshot").cloned(),
                                );
                                return (runtime, typed_operation_resp(result));
                            }
                            let operation = self.record_incomplete_operation(
                                &context,
                                &request,
                                admitted,
                                nested.or_else(|| outer_binding.bound()),
                                outer_binding.bound().is_some(),
                                business_receipts.clone(),
                            );
                            let mut result = operation_result(
                                &request,
                                operation.clone(),
                                false,
                                response.data.get("snapshot").cloned(),
                            );
                            result.failed_phase = Some(operation.phase.as_str().to_owned());
                            result.requires.kind = Some("repair".into());
                            result.owner_readback.insert("master_grant".into(), json!({"expected":"approved grant replacement committed and read back","observed":reason}));
                            return (runtime, typed_operation_resp(result));
                        }
                        grant_committed = true;
                    } else if !self.identity_only_grant_retained(&context, &response, approved) {
                        let operation = self.record_incomplete_operation(
                            &context,
                            &request,
                            admitted,
                            nested.or_else(|| outer_binding.bound()),
                            outer_binding.bound().is_some(),
                            business_receipts.clone(),
                        );
                        let mut result = operation_result(
                            &request,
                            operation.clone(),
                            false,
                            response.data.get("snapshot").cloned(),
                        );
                        result.failed_phase = Some(operation.phase.as_str().to_owned());
                        result.requires.kind = Some("repair".into());
                        result.owner_readback.insert("master_grant".into(), json!({"expected":"existing grant retained for the recovered identity","observed":"readback mismatch"}));
                        return (runtime, typed_operation_resp(result));
                    }
                }
                // The authority receipt sits between `credential` and `lease`
                // in the accepted order, from either successful authority owner.
                if grant_committed && !business_receipts.iter().any(|receipt| receipt == "grant") {
                    let position = business_receipts
                        .iter()
                        .position(|receipt| receipt == "lease")
                        .unwrap_or(business_receipts.len());
                    business_receipts.insert(position, "grant".to_owned());
                }
                // The terminal receipt is persisted only when every required
                // owner already proved its durable result. A missing lease, or
                // a required grant that did not commit, never becomes
                // `context_complete`; the operation is then projected as
                // incomplete below instead of inferring success from the
                // lifecycle `completed` marker.
                let lease_proven = business_receipts.iter().any(|receipt| receipt == "lease");
                let grant_required = approved
                    .as_ref()
                    .is_some_and(ApprovedAdmission::requires_grant_replacement);
                let context_complete = lease_proven && (!grant_required || grant_committed);
                if context_complete {
                    business_receipts.push("context_complete".to_owned());
                }
                if !context_complete {
                    // The Register owner returned ok but a required owner
                    // receipt is still missing. Keep only the proved receipts
                    // and report a truthful incomplete result; never translate
                    // the outer lifecycle `completed` marker into business
                    // success.
                    let operation = self.record_incomplete_operation(
                        &context,
                        &request,
                        admitted,
                        nested.or_else(|| outer_binding.bound()),
                        outer_binding.bound().is_some(),
                        business_receipts,
                    );
                    let mut result = operation_result(
                        &request,
                        operation.clone(),
                        false,
                        response.data.get("snapshot").cloned(),
                    );
                    result.failed_phase = Some(operation.phase.as_str().to_owned());
                    result.requires.kind = Some("repair".into());
                    return (runtime, typed_operation_resp(result));
                }
                let operation = match self.finish_identity_operation(
                    &context,
                    &request,
                    nested,
                    outer_binding.bound().is_some(),
                    business_receipts.clone(),
                ) {
                    Ok(operation) => operation,
                    Err(error) => {
                        // The owner reached a durable outcome but the outer
                        // phase chain could not be advanced. Project only the
                        // last durable phase; never invent completed receipt.
                        let fallback = self
                            .latest_projection(&context, &request)
                            .unwrap_or(admitted);
                        let mut result = operation_result(
                            &request,
                            fallback.clone(),
                            fallback.outcome == "completed",
                            response.data.get("snapshot").cloned(),
                        );
                        result.failed_phase = (fallback.outcome != "completed")
                            .then(|| fallback.phase.as_str().to_owned());
                        if fallback.outcome != "completed" {
                            result.requires.kind = Some("repair".into());
                        }
                        result.owner_readback.insert("error".into(), json!(error));
                        return (runtime, typed_operation_resp(result));
                    }
                };
                let result = operation_result(
                    &request,
                    operation,
                    true,
                    response.data.get("snapshot").cloned(),
                );
                self.operation_journal.remove_live(&request.operation_id);
                (runtime, typed_operation_resp(result))
            }
            Ok((runtime, response, nested)) => {
                let bound = outer_binding.bound();
                let awaiting_reply = response
                    .error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("REGISTER_REPLY_PENDING"));
                // The prepared envelope's IDs are durable but consume produced
                // no receipt (for example the existing CAS rejected a stale
                // revision). The outer projection must stay at the durable
                // `Validating` intent: `unknown`, never `inner_dispatched`
                // or `partial`, and no reprepare/retry.
                if nested.is_none() && (outer_binding.bound().is_some() || awaiting_reply) {
                    if awaiting_reply {
                        self.operation_journal
                            .reopen_safe(&request.operation_id, Vec::new());
                    } else {
                        self.operation_journal.remove_live(&request.operation_id);
                    }
                    let fallback = self
                        .latest_projection(&context, &request)
                        .unwrap_or_else(|| admitted.clone());
                    let mut result = operation_result(
                        &request,
                        fallback,
                        false,
                        response.data.get("snapshot").cloned(),
                    );
                    if let Some(message) = response.error {
                        let mut readback = serde_json::Map::new();
                        readback.insert("error".into(), serde_json::Value::String(message));
                        result.owner_readback = readback;
                    }
                    return (runtime, typed_operation_resp(result));
                }
                self.operation_journal.remove_live(&request.operation_id);
                // Reconciliation failed after admission. A committed nested
                // Register receipt makes this a `partial` with committed
                // phases retained; otherwise it is a pre-commit `failed`.
                let operation = self.record_incomplete_operation(
                    &context,
                    &request,
                    admitted,
                    nested.clone().or_else(|| bound.clone()),
                    outer_binding.bound().is_some(),
                    business_receipts_for_failure(
                        approved.as_ref(),
                        nested.as_ref().or(bound.as_ref()),
                    ),
                );
                let mut result = operation_result(
                    &request,
                    operation.clone(),
                    false,
                    response.data.get("snapshot").cloned(),
                );
                if operation.outcome == "partial" {
                    if let Some(message) = response.error {
                        let mut readback = serde_json::Map::new();
                        readback.insert("error".into(), serde_json::Value::String(message));
                        result.owner_readback = readback;
                    }
                } else {
                    result.failed_phase = Some(operation.phase.as_str().to_owned());
                    if let Some(message) = response.error {
                        let mut readback = serde_json::Map::new();
                        readback.insert("error".into(), serde_json::Value::String(message));
                        result.owner_readback = readback;
                    }
                }
                (runtime, typed_operation_resp(result))
            }
            Err(error) => {
                self.operation_journal.remove_live(&request.operation_id);
                let bound = outer_binding.bound();
                let operation = self.record_incomplete_operation(
                    &context,
                    &request,
                    admitted,
                    error.nested.clone().or_else(|| bound.clone()),
                    outer_binding.bound().is_some(),
                    business_receipts_for_failure(
                        approved.as_ref(),
                        error.nested.as_ref().or(bound.as_ref()),
                    ),
                );
                let mut result = operation_result(&request, operation.clone(), false, None);
                result.failed_phase = Some(operation.phase.as_str().to_owned());
                result.owner_readback = {
                    let mut readback = serde_json::Map::new();
                    readback.insert(
                        "error".into(),
                        serde_json::Value::String(format!("{:#}", error.error)),
                    );
                    readback
                };
                (self.host.clone(), typed_operation_resp(result))
            }
        }
    }

    pub(super) fn participant_register_reply(
        &self,
        context: ProjectContext,
        reply: crate::proto::RegisterParticipantReply,
    ) -> (Arc<Server>, Resp) {
        if self.operation_journal.is_cancelled(&reply.operation_id) {
            return (self.host.clone(), Resp::err("IDENTITY_OPERATION_CANCELLED"));
        }
        let record =
            match register_reply::accept(self.host.host_paths.state_root(), &context, &reply) {
                Ok(record) => record,
                Err(error) => return (self.host.clone(), Resp::err(error)),
            };
        let Some(continuation) = record.continuation.clone() else {
            return (
                self.host.clone(),
                Resp::err("COMMUNICATION_RESTORE_CONTINUATION_MISSING"),
            );
        };
        // The exact admitted intent and prepared envelope come from the one
        // control owner, never from return payload or query projection.
        // The initiating CLI has exited by the time its participant replies.
        // Continue from the selected transport just verified by accept(),
        // while keeping the original request intact for operation idempotence.
        let mut facts = continuation.request.facts.clone();
        facts.caller_pid = None;
        facts.pane_hint = None;
        if record.transport.kind == TransportKind::Tmux {
            facts.tmux = record.transport.tmux_endpoint.clone().map(|endpoint| {
                crate::proto::TmuxCandidate {
                    endpoint,
                    cwd: continuation.context.canonical_root.clone(),
                }
            });
            facts.tmux_anchor = None;
        }
        self.identity_context_continue(
            continuation.context,
            continuation.request,
            true,
            Some(facts),
        )
    }

    /// Read current typed owners without loading a runtime or replaying any
    /// registration, credential, lease, role or presence-maintenance effect.
    fn completed_context_readback(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
    ) -> anyhow::Result<(Arc<Server>, serde_json::Value)> {
        if context.app_scope_id.as_str() != identity::CLI_APP_SERVER_ID
            || context.runtime_context.is_some()
        {
            anyhow::bail!(
                "bootstrap requires the CLI project scope without a caller-selected identity"
            );
        }
        let mut facts = facts.clone();
        facts = self.observe_identity_facts(context, &facts)?;
        self.complete_persisted_tmux_facts(context, &mut facts)?;
        validate_facts(&facts)?;
        let scope = Scope {
            root: PathBuf::from(&context.canonical_root),
        };
        // Read retained state by the caller's anchor. A readback must not
        // manufacture current communication facts from the saved transport.
        let host_binding = self.current_anchor_binding(context, &facts)?;
        let (app_scope, binding_id) = if let Some(binding) = host_binding.as_ref() {
            if binding.project_scope != context.project_scope {
                anyhow::bail!("current anchor belongs to another project");
            }
            (binding.app_scope_id.clone(), binding.binding_id.clone())
        } else {
            let persisted =
                identity::existing_for_daemon_at(&self.host.host_paths, &scope, &facts)?;
            let saved = persisted
                .as_ref()
                .and_then(|identity| identity.runtime.as_ref())
                .ok_or_else(|| anyhow::anyhow!("current anchor has no typed runtime binding"))?;
            if persisted
                .as_ref()
                .and_then(|identity| identity.project_scope.as_ref())
                != Some(&context.project_scope)
            {
                anyhow::bail!("persisted anchor belongs to another project");
            }
            (saved.appserver_id.clone(), saved.binding_id.clone())
        };
        let route_scope = crate::scope::RouteScope {
            app_scope_id: app_scope.clone(),
            project_scope_id: context.project_scope.clone(),
        };
        let key = (
            app_scope.as_str().to_owned(),
            context.project_scope.as_str().to_owned(),
        );
        let runtime = self
            .routes
            .lock()
            .unwrap()
            .get(&key)
            .and_then(|route| route.runtime.clone())
            .ok_or_else(|| anyhow::anyhow!("current project runtime is not loaded"))?;
        let (binding, worker, target) = {
            let state = runtime.state.lock().unwrap();
            let binding = state
                .global
                .lookup_binding_for(&route_scope, &binding_id)
                .ok_or_else(|| anyhow::anyhow!("current project binding is missing"))?
                .clone();
            if host_binding.as_ref().is_some_and(|host| host != &binding) {
                anyhow::bail!("current host and project bindings differ");
            }
            let worker = state
                .workers
                .get(binding.agent_id.as_str())
                .ok_or_else(|| anyhow::anyhow!("current binding worker is missing"))?
                .clone();
            let transport = selected_transport_for_worker(&worker)
                .ok_or_else(|| anyhow::anyhow!("current worker selected transport is missing"))?;
            if transport.tmux_endpoint != binding.tmux_endpoint
                || (matches!(transport.kind, TransportKind::AppServer | TransportKind::DesktopBridge)
                    && transport.thread_id.as_deref()
                        != binding
                            .native_thread_id
                            .as_ref()
                            .map(NativeThreadId::as_str))
            {
                anyhow::bail!("current selected transport differs from binding");
            }
            let target = close_endpoint_target(&state, &worker.id);
            (binding, worker, target)
        };
        let response = context_projection(&runtime, worker.id.clone(), worker.token.clone());
        if !response.ok {
            anyhow::bail!(
                "current context projection failed: {}",
                response.error.unwrap_or_default()
            );
        }
        if response.data.get("binding") != Some(&json!(binding)) {
            anyhow::bail!("current context binding is ambiguous or changed");
        }
        let current_target = {
            let state = runtime.state.lock().unwrap();
            close_endpoint_target(&state, &worker.id)
        };
        if current_target != target
            || (host_binding.is_some()
                && self.current_anchor_binding(context, &facts)? != host_binding)
        {
            anyhow::bail!("current readback owner or binding changed");
        }
        let mut snapshot = response.data;
        name_resolution::project_registered_name(&runtime, &worker.id, &mut snapshot);
        add_optional_master_choice(&mut snapshot);
        Ok((runtime, snapshot))
    }

    fn resolve_context_name(
        &self,
        runtime: &Server,
        context: &ProjectContext,
        request: &IdentityContextRequest,
        response: &mut Resp,
    ) -> Result<name_resolution::NameResolutionEffects, String> {
        let identity: identity::Identity =
            serde_json::from_value(response.data["identity_receipt"].clone())
                .map_err(|error| format!("NAME_IDENTITY_REQUIRED: {error}"))?;
        let control = name_adjudication_control(request)
            .map_err(|ApprovedAdmissionError::Error(error)| error)?;
        let mut intent = name_resolution::parse_name_intent(
            request
                .grant_approval
                .as_ref()
                .or(request.approval.as_ref()),
        )?;
        if request
            .grant_approval
            .as_ref()
            .or(request.approval.as_ref())
            .is_some_and(|approval| approval["action"] == "restore_peer")
        {
            let state = runtime.state.lock().unwrap();
            let name = name_resolution::available_ordinary_name(
                &state,
                context.project_scope.as_str(),
                &identity.worker_id,
            )?;
            intent = Some(name_resolution::NameIntent {
                requested_name: name,
                user_authorized: true,
                replace_confirmed: false,
            });
        }
        let fallback = identity
            .runtime
            .as_ref()
            .map(|binding| {
                format!(
                    "context-{}-{}",
                    identity.worker_id, binding.endpoint_generation
                )
            })
            .unwrap_or_default();
        let operation = if request.operation_id.is_empty() {
            fallback.as_str()
        } else {
            &request.operation_id
        };
        let outcome = if let Some((operation, verdict, evidence)) = control {
            let adjudication = super::name_stewardship::adjudicate(
                runtime,
                &identity.worker_id,
                &identity.token,
                &operation,
                verdict,
                &evidence,
            );
            if !adjudication.ok {
                let error = adjudication
                    .error
                    .unwrap_or_else(|| "NAME_ADJUDICATION_FAILED".into());
                response.data["snapshot"]["name_resolution"] =
                    json!({"outcome":"failed", "operation_id":operation, "error":error});
                return Err(error);
            }
            adjudication.data
        } else {
            name_resolution::resolve_registered_name_with_authorization(
                runtime,
                &identity.worker_id,
                operation,
                &request.invocation,
                intent,
                request
                    .grant_approval
                    .as_ref()
                    .or(request.approval.as_ref())
                    .and_then(|approval| approval.get("authorization"))
                    .and_then(serde_json::Value::as_str),
            )?
        };
        let effects = serde_json::from_value(outcome["effects"].clone())
            .map_err(|error| format!("NAME_EFFECTS_INVALID: {error}"))?;
        let binding = identity
            .runtime
            .as_ref()
            .ok_or("NAME_IDENTITY_REQUIRED: runtime missing")?;
        let registered =
            ProjectContext::for_registered_route(Path::new(&context.canonical_root), binding)
                .map_err(|error| error.to_string())?;
        let refreshed = self
            .dispatch_sync(
                Some(registered),
                Req::Context {
                    worker_id: identity.worker_id.clone(),
                    token: identity.token.clone(),
                },
            )
            .1;
        if !refreshed.ok {
            return Err(refreshed
                .error
                .unwrap_or_else(|| "NAME_READBACK_FAILED".into()));
        }
        response.data["snapshot"] = refreshed.data;
        name_resolution::project_registered_name(
            runtime,
            &identity.worker_id,
            &mut response.data["snapshot"],
        );
        response.data["snapshot"]["name_resolution"] = outcome;
        add_optional_master_choice(&mut response.data["snapshot"]);
        Ok(effects)
    }

    pub(super) fn name_recovery_readback(
        &self,
        context: &ProjectContext,
        operation_id: &str,
    ) -> Option<serde_json::Value> {
        let runtime = self
            .routes
            .lock()
            .unwrap()
            .get(&Self::route_key(context))
            .and_then(|route| route.runtime.clone())?;
        let state = runtime.state.lock().unwrap();
        let recovery = state.name_recovery(operation_id)?;
        if recovery.project_scope != context.project_scope.as_str() {
            return None;
        }
        Some(
            json!({"outcome":recovery.progress, "operation_id":operation_id, "recovery":recovery,
            "effects":name_resolution::NameResolutionEffects::from_recovery(recovery)}),
        )
    }

    fn record_name_pending(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
        admitted: crate::proto::IdentityOperationProjection,
        receipts: Vec<String>,
    ) -> crate::proto::IdentityOperationProjection {
        use crate::proto::IdentityOperationPhase as Phase;
        let mut current = self.latest_projection(context, request).unwrap_or(admitted);
        for (from, to) in [
            (Phase::Admitted, Phase::Validating),
            (Phase::Validating, Phase::InnerDispatched),
            (Phase::InnerDispatched, Phase::EffectObserved),
        ] {
            if current.phase == from
                && !self.advance_operation(
                    request,
                    &mut current,
                    to,
                    None,
                    None,
                    Some(receipts.clone()),
                )
            {
                break;
            }
        }
        self.operation_journal.remove_live(&request.operation_id);
        current
    }

    fn authorize_bootstrap_policy(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
    ) -> (Arc<Server>, Resp) {
        let result = (|| -> Result<(Arc<Server>, Resp), String> {
            let raw = request.grant_approval.as_ref().unwrap();
            if raw
                .get("decision")
                .is_some_and(|decision| decision != "approved")
            {
                return Err("BOOTSTRAP_APPROVAL_NOT_APPROVED".into());
            }
            let authorization = raw["authorization"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or("BOOTSTRAP_AUTHORIZATION_REQUIRED")?;
            for (field, expected) in [
                ("project_scope", context.project_scope.as_str()),
                ("app_scope_id", context.app_scope_id.as_str()),
            ] {
                if raw
                    .get(field)
                    .is_some_and(|value| value.as_str() != Some(expected))
                {
                    return Err("BOOTSTRAP_APPROVAL_SCOPE_MISMATCH".into());
                }
            }
            if request.approval.is_some() {
                return Err("BOOTSTRAP_POLICY_ONLY: submit registration separately".into());
            }
            let runtime = self.select_runtime(context)?;
            let mut state = runtime.state.lock().unwrap();
            if state
                .global
                .lookup_project(&context.project_scope)
                .is_some_and(|project| {
                    project
                        .bootstrap_policies
                        .contains_key(context.app_scope_id.as_str())
                })
            {
                return Err("BOOTSTRAP_POLICY_ALREADY_AUTHORIZED".into());
            }
            let admitted = self.admit_identity_operation(context, request)?;
            let outer = RegisterOuterBinding::new(
                self.operation_journal.clone(),
                request.operation_id.clone(),
            );
            let command_id = format!("bootstrap-{}", request.operation_id);
            let operation_id = format!("bootstrap-{}", request.operation_id);
            outer.bind_validating_ids(&command_id, &operation_id)?;
            self.operation_journal
                .begin_owner_start(&request.operation_id, "bootstrap_policy")?;
            let policy = global_state::BootstrapPolicy {
                project_scope: context.project_scope.clone(),
                app_scope_id: context.app_scope_id.clone(),
                authorization: authorization.to_owned(),
                authorized_operation: request.operation_id.clone(),
                consumption: None,
            };
            let committed = runtime.commit_command_locked(
                &mut state,
                &command_id,
                &operation_id,
                &[Event::GlobalBootstrapPolicySet { policy }],
                json!({"bootstrap_policy":"authorized"}),
                None,
                Some(&outer),
            );
            drop(state);
            if let Err(error) = committed {
                let operation = self.record_incomplete_operation(
                    context,
                    request,
                    admitted,
                    outer.bound(),
                    false,
                    Vec::new(),
                );
                let mut result = operation_result(request, operation, false, None);
                result.owner_readback.insert(
                    "bootstrap_policy".into(),
                    json!({"error":error.to_string()}),
                );
                return Ok((runtime, typed_operation_resp(result)));
            }
            let operation = self.finish_identity_operation(
                context,
                request,
                Some((command_id, operation_id)),
                true,
                vec!["bootstrap_policy".into(), "context_complete".into()],
            )?;
            self.operation_journal.remove_live(&request.operation_id);
            Ok((
                runtime,
                typed_operation_resp(operation_result(request, operation, true, None)),
            ))
        })();
        result.unwrap_or_else(|error| (self.host.clone(), Resp::err(error)))
    }

    fn manual_communication_gate(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
        outer: &RegisterOuterBinding,
        reconciliation: Result<(Arc<Server>, Resp, Option<(String, String)>), ReconcileError>,
    ) -> Result<(Arc<Server>, Resp, Option<(String, String)>), ReconcileError> {
        let (runtime, mut response, nested) = reconciliation?;
        if !response.ok {
            return Ok((runtime, response, nested));
        }
        let identity: identity::Identity =
            serde_json::from_value(response.data["identity_receipt"].clone()).map_err(|error| {
                anyhow::anyhow!("COMMUNICATION_RESTORE_IDENTITY_REQUIRED: {error}")
            })?;
        let binding = identity
            .runtime
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("COMMUNICATION_RESTORE_IDENTITY_REQUIRED"))?;
        let transport = identity
            .transport
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("COMMUNICATION_RESTORE_BINDING_REQUIRED"))?;
        if transport.kind == TransportKind::Dsh {
            // DSH restoration has already verified the current host facts and
            // the exact receive-loop enrollment receipt. Restoring a binding
            // does not ask the agent to answer a new communication challenge.
            // This does not fabricate message delivery/consumption evidence.
            return Ok((runtime, response, nested));
        }
        let root = self.host.host_paths.state_root();
        let saved =
            register_reply::read(root, &request.operation_id).map_err(anyhow::Error::msg)?;
        let prepared = match saved.as_ref() {
            Some(record) => record.prepared.clone(),
            None => runtime
                .prepare_register_envelope_for_scope(
                    &identity.worker_id,
                    &identity.token,
                    transport,
                    context.project_scope.clone(),
                    &context.canonical_root,
                    binding.appserver_id.clone(),
                    true,
                )
                .map_err(anyhow::Error::msg)?,
        };
        let gate = register_reply::ensure_pending(
            root,
            &identity.worker_id,
            transport,
            &prepared,
            Some(outer),
            register_reply::Purpose::ManualCommunicationRestore,
        )
        .and_then(|record| {
            register_reply::enforce(root, record.clone())?;
            register_reply::verified_for_commit(root, &record)
        });
        if let Err(error) = gate {
            response.ok = false;
            response.error = Some(error);
        }
        // Communication evidence changes only after a real reply. No Register,
        // credential or authority mutation occurs on this restored binding.
        Ok((runtime, response, nested))
    }

    fn prepare_context_invocation(
        &self,
        context: &ProjectContext,
        request: IdentityContextRequest,
    ) -> (Arc<Server>, Resp) {
        if request.query
            || request.invocation == "query"
            || request.action != "prepare_invocation"
            || request.query_capability.trim().is_empty()
        {
            return (
                self.host.clone(),
                Resp::err("IDENTITY_OPERATION_PREPARE_SHAPE_INVALID"),
            );
        }
        let mut intent = request.clone();
        intent.action = "context".into();
        let digest = identity_operation_intent_digest(context, &intent);
        match self.operation_journal.prepare_invocation(
            &request.operation_id,
            context.project_scope.as_str(),
            context.app_scope_id.as_str(),
            "context",
            &request.invocation,
            &digest,
            &crate::server::operation_journal::capability_hash(&request.query_capability),
        ) {
            Ok(ticket) => (
                self.host.clone(),
                Resp::data(json!({
                    "operation_id": request.operation_id,
                    "invocation_ticket": ticket
                })),
            ),
            Err(error) => (self.host.clone(), Resp::err(error)),
        }
    }

    fn cancel_context_operation(
        &self,
        context: &ProjectContext,
        request: IdentityContextRequest,
    ) -> (Arc<Server>, Resp) {
        if request.query
            || !request.facts.eq(&IdentityFacts::default())
            || request.approval.is_some()
            || request.grant_approval.is_some()
            || request.query_capability.trim().is_empty()
            || request.operation_id.trim().is_empty()
        {
            return (
                self.host.clone(),
                Resp::err("IDENTITY_OPERATION_CANCEL_SHAPE_INVALID"),
            );
        }
        if identity::validate_id_for_protocol(&request.operation_id).is_err() {
            return (
                self.host.clone(),
                Resp::err("IDENTITY_OPERATION_ID_INVALID"),
            );
        }
        let capability_hash =
            crate::server::operation_journal::capability_hash(&request.query_capability);
        match self.operation_journal.cancel(
            &request.operation_id,
            context.project_scope.as_str(),
            context.app_scope_id.as_str(),
            &capability_hash,
        ) {
            Ok(crate::server::operation_journal::CancelOutcome::NotAdmitted) => (
                self.host.clone(),
                Resp::data(json!({
                    "cancellation": {
                        "operation_id": request.operation_id,
                        "disposition": "not_admitted_cancelled"
                    }
                })),
            ),
            Ok(outcome) => {
                let ack = crate::server::operation_journal::OperationJournal::cancel_ack(outcome);
                (
                    self.host.clone(),
                    Resp::data(json!({ "cancellation": ack })),
                )
            }
            Err(error) => (self.host.clone(), Resp::err(error)),
        }
    }

    fn preflight_identity_facts(
        &self,
        context: &ProjectContext,
        mut facts: IdentityFacts,
    ) -> Result<IdentityFacts, PreflightIdentityError> {
        if context.app_scope_id.as_str() != identity::CLI_APP_SERVER_ID
            || context.runtime_context.is_some()
        {
            return Err(PreflightIdentityError::Error(anyhow::anyhow!(
                "IDENTITY_CONTEXT_INVALID: bootstrap requires the CLI project scope without a caller-selected identity"
            )));
        }
        let root = PathBuf::from(&context.canonical_root);
        context
            .validate_registered_root(&root)
            .map_err(PreflightIdentityError::Error)?;
        validate_project_registration_cwd(&context.canonical_root, &root)
            .map_err(|error| PreflightIdentityError::Error(anyhow::Error::msg(error)))?;
        facts = self
            .observe_identity_facts(context, &facts)
            .map_err(PreflightIdentityError::Error)?;
        self.complete_persisted_tmux_facts(context, &mut facts)
            .map_err(PreflightIdentityError::Error)?;
        validate_facts(&facts).map_err(PreflightIdentityError::Error)?;
        // Preserve the existing pane recovery contract. Native communication
        // facts must come from this invocation, never the saved transport.
        let mut required = if self.has_persisted_pane_route(context, &facts)
            || self.has_current_desktop_binding(context, &facts).map_err(PreflightIdentityError::Error)? {
            Vec::new()
        } else {
            required_fields(&facts)
        };
        if facts.tmux.is_none() && facts.dsh_session_id.is_some() && facts.dsh.is_none()
            && identity::existing_for_daemon_at(&self.host.host_paths, &Scope { root: root.clone() }, &facts)
                .map_err(PreflightIdentityError::Error)?.is_none() {
            if self.current_session_identity(context, &facts).map_err(PreflightIdentityError::Error)?.is_none() {
                required.push("dsh");
            }
        }
        if !required.is_empty() {
            let descriptions = json!({
                "dsh": "Current DSH gateway endpoint, runtime_id, agent_id, session_id and cwd",
                "session_id": "Current runtime session identifier",
                "thread_id": "Current native thread identifier",
                "endpoint": "Current native AppServer unix socket endpoint",
                "namespace": "Current native endpoint namespace: codex_app or codex_tui"
            });
            let missing_descriptions: serde_json::Map<String, serde_json::Value> = required
                .iter()
                .map(|field| ((*field).to_owned(), descriptions[*field].clone()))
                .collect();
            return Err(PreflightIdentityError::MissingFacts(json!({
                "snapshot": {
                    "registered": false,
                    "identity": null,
                    "requires_identity_update": {
                        "required": true,
                        "reason": "IDENTITY_INFORMATION_REQUIRED",
                        "required_fields": required,
                        "field_descriptions": missing_descriptions,
                        "action": "collab context --provide '<JSON containing required_fields>'",
                        "requires_approval": false
                    }
                },
                "identity_receipt": null
            })));
        }
        Ok(facts)
    }

    fn finish_identity_operation(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
        nested: Option<(String, String)>,
        already_validated: bool,
        business_receipts: Vec<String>,
    ) -> Result<crate::proto::IdentityOperationProjection, String> {
        use crate::proto::IdentityOperationPhase;
        let operation_id = request.operation_id.as_str();
        let (nested_command_id, nested_operation_id) = nested
            .map(|(command_id, operation_id)| (Some(command_id), Some(operation_id)))
            .unwrap_or((None, None));
        let mut operation = self.latest_projection(context, request).ok_or_else(|| {
            "IDENTITY_OPERATION_UNKNOWN: durable outer operation is unavailable".to_string()
        })?;
        if let (Some(command_id), Some(operation_id)) =
            (nested_command_id.as_ref(), nested_operation_id.as_ref())
        {
            if operation
                .nested_command_id
                .as_ref()
                .is_some_and(|existing| existing != command_id)
                || operation
                    .nested_operation_id
                    .as_ref()
                    .is_some_and(|existing| existing != operation_id)
            {
                return Err(
                    "IDENTITY_OPERATION_PHASE_CONFLICT: nested receipt IDs changed".to_string(),
                );
            }
        }
        if operation.phase == IdentityOperationPhase::Admitted {
            operation = self.operation_journal.transition(
                operation_id,
                IdentityOperationPhase::Validating,
                nested_command_id.clone(),
                nested_operation_id.clone(),
            )?;
        } else if !already_validated
            && operation.phase == IdentityOperationPhase::Validating
            && operation.nested_command_id.is_none()
        {
            return Err(
                "IDENTITY_OPERATION_PHASE_CONFLICT: validating phase has no nested receipt"
                    .to_string(),
            );
        }
        if already_validated && operation.phase == IdentityOperationPhase::Validating {
            return Err(
                "IDENTITY_OPERATION_UNKNOWN: Register Start promotion is not durable".into(),
            );
        }
        if operation.phase == IdentityOperationPhase::Validating {
            operation = self.operation_journal.transition(
                operation_id,
                IdentityOperationPhase::InnerDispatched,
                nested_command_id.clone(),
                nested_operation_id.clone(),
            )?;
        }
        if operation.nested_command_id.is_none()
            && !business_receipts
                .iter()
                .any(|receipt| receipt == "context_complete")
        {
            // No nested Register receipt exists for this invocation, so no
            // effect-producing phase can be claimed; the durable phase stops at
            // `validating` and the projection reports it honestly.
            return Ok(operation);
        }
        if operation.phase == IdentityOperationPhase::InnerDispatched {
            operation = self.operation_journal.transition_with_business_receipts(
                operation_id,
                IdentityOperationPhase::EffectObserved,
                None,
                None,
                Some(business_receipts.clone()),
            )?;
        }
        if operation.phase != IdentityOperationPhase::EffectObserved {
            return Err(format!(
                "IDENTITY_OPERATION_PHASE_CONFLICT: cannot complete from {}",
                operation.phase.as_str()
            ));
        }
        self.operation_journal.transition_with_business_receipts(
            operation_id,
            IdentityOperationPhase::Completed,
            None,
            None,
            Some(business_receipts),
        )
    }

    /// Read the last durable projection for this exact key/capability/scope.
    /// Read-only: the journal query compares the capability hash and never
    /// dispatches a side effect.
    fn latest_projection(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
    ) -> Option<crate::proto::IdentityOperationProjection> {
        self.operation_journal
            .query(
                request,
                context.project_scope.as_str(),
                context.app_scope_id.as_str(),
            )
            .ok()
            .map(|envelope| envelope.result)
    }

    /// Record the truthful terminal phase after an admitted operation could
    /// not complete. A committed nested Register receipt makes the operation
    /// `partial` with committed phases retained; without one it is a
    /// pre-commit `failed`. If a transition cannot be appended, the last
    /// durable projection is returned instead of an invented receipt.
    fn record_incomplete_operation(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
        admitted: crate::proto::IdentityOperationProjection,
        nested: Option<(String, String)>,
        already_validated: bool,
        business_receipts: Vec<String>,
    ) -> crate::proto::IdentityOperationProjection {
        use crate::proto::IdentityOperationPhase;
        let mut current = self.latest_projection(context, request).unwrap_or(admitted);
        let nested = nested.or_else(|| {
            current
                .nested_command_id
                .clone()
                .zip(current.nested_operation_id.clone())
        });
        let Some((command_id, operation_id)) = nested else {
            let ok = self.advance_operation(
                request,
                &mut current,
                IdentityOperationPhase::Failed,
                None,
                None,
                Some(business_receipts),
            );
            return if ok {
                current
            } else {
                self.latest_projection(context, request).unwrap_or(current)
            };
        };
        if current.phase == IdentityOperationPhase::Admitted {
            if !self.advance_operation(
                request,
                &mut current,
                IdentityOperationPhase::Validating,
                Some(command_id.clone()),
                Some(operation_id.clone()),
                None,
            ) {
                return self.latest_projection(context, request).unwrap_or(current);
            }
        }
        if current.phase == IdentityOperationPhase::Validating {
            if already_validated {
                return self.latest_projection(context, request).unwrap_or(current);
            }
            if !self.advance_operation(
                request,
                &mut current,
                IdentityOperationPhase::InnerDispatched,
                Some(command_id),
                Some(operation_id),
                None,
            ) {
                return self.latest_projection(context, request).unwrap_or(current);
            }
        }
        if current.phase == IdentityOperationPhase::InnerDispatched {
            if !self.advance_operation(
                request,
                &mut current,
                IdentityOperationPhase::EffectObserved,
                None,
                None,
                Some(business_receipts.clone()),
            ) {
                return self.latest_projection(context, request).unwrap_or(current);
            }
        }
        if matches!(
            current.phase,
            IdentityOperationPhase::EffectObserved
                | IdentityOperationPhase::InnerDispatched
                | IdentityOperationPhase::Validating
        ) {
            if !self.advance_operation(
                request,
                &mut current,
                IdentityOperationPhase::Partial,
                None,
                None,
                Some(business_receipts),
            ) {
                return self.latest_projection(context, request).unwrap_or(current);
            }
        }
        current
    }

    fn advance_operation(
        &self,
        request: &IdentityContextRequest,
        current: &mut crate::proto::IdentityOperationProjection,
        phase: crate::proto::IdentityOperationPhase,
        nested_command_id: Option<String>,
        nested_operation_id: Option<String>,
        business_receipts: Option<Vec<String>>,
    ) -> bool {
        match self.operation_journal.transition_with_business_receipts(
            &request.operation_id,
            phase,
            nested_command_id,
            nested_operation_id,
            business_receipts,
        ) {
            Ok(next) => {
                *current = next;
                true
            }
            Err(_) => false,
        }
    }

    fn admit_identity_operation(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
    ) -> Result<crate::proto::IdentityOperationProjection, String> {
        if request.query {
            return Err("IDENTITY_OPERATION_QUERY_SHAPE_INVALID".into());
        }
        if !matches!(
            request.invocation.as_str(),
            "automatic" | "supplement" | "approved_recovery" | "manual_communication_restore"
        ) {
            return Err("IDENTITY_CONTEXT_UNSUPPORTED: unsupported context invocation".into());
        }
        if request.action != "context" {
            return Err("IDENTITY_OPERATION_INTENT_INVALID: action must be context".into());
        }
        if request.query_capability.trim().is_empty() {
            return Err("IDENTITY_OPERATION_CAPABILITY_REQUIRED: mutating context requires a query capability".into());
        }
        identity::validate_id_for_protocol(&request.operation_id)
            .map_err(|error| format!("IDENTITY_OPERATION_ID_INVALID: {error}"))?;
        let digest = identity_operation_intent_digest(context, request);
        let admission = self.operation_journal.append(
            crate::server::operation_journal::OperationAdmission {
                operation_id: request.operation_id.clone(),
                project_scope: context.project_scope.as_str().to_owned(),
                app_scope_id: context.app_scope_id.as_str().to_owned(),
                action: request.action.clone(),
                invocation: request.invocation.clone(),
                intent_digest: digest,
                query_capability_hash: crate::server::operation_journal::capability_hash(
                    &request.query_capability,
                ),
                phase: crate::proto::IdentityOperationPhase::Admitted,
                committed_phases: Vec::new(),
                nested_command_id: None,
                nested_operation_id: None,
                approval_evidence: (request.approval.is_some() || request.grant_approval.is_some())
                    .then(|| {
                        json!({
                            "identity": request.approval,
                            "grant": request.grant_approval,
                        })
                    }),
            },
        )?;
        self.operation_journal
            .install_safe(&request.operation_id, Vec::new());
        Ok(admission)
    }
}
