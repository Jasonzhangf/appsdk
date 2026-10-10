impl ProjectRuntimeManager {
    pub(super) fn identity_context(
        &self,
        context: ProjectContext,
        request: IdentityContextRequest,
    ) -> (Arc<Server>, Resp) {
        if request.action == "prepare_invocation" {
            return self.prepare_context_invocation(&context, request);
        }
        if request.action == "cancel" {
            return self.cancel_context_operation(&context, request);
        }
        let _guard = self.identity_gate.lock().unwrap();
        if request.operation_id.is_empty() {
            return match self.reconcile_identity_context(context, request.facts, None, None) {
                Ok((runtime, response, _)) => (runtime, response),
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
            return match self.admit_identity_operation(&context, &request) {
                Ok(_) => {
                    let completed = existing.result.outcome == "completed";
                    let result = operation_result(&request, existing.result, completed, None);
                    (self.host.clone(), typed_operation_resp(result))
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
        let digest = identity_operation_intent_digest(&context, &request);
        if let Err(error) = self.operation_journal.claim_invocation(
            &request.operation_id,
            &request.invocation_ticket,
            &digest,
            &crate::server::operation_journal::capability_hash(&request.query_capability),
        ) {
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
        // New approved recovery requires both owner fences before durable
        // admission. A same-key retry above returns its retained projection;
        // it must not re-evaluate an incumbent already changed by that commit.
        let approved = match self.approved_recovery_admission(&context, &request) {
            Ok(approved) => approved,
            Err(ApprovedAdmissionError::Denied(result)) => {
                return (self.host.clone(), typed_operation_resp(result));
            }
            Err(ApprovedAdmissionError::Error(error)) => {
                return (self.host.clone(), Resp::err(error));
            }
        };
        let facts = match self.preflight_identity_facts(&context, request.facts.clone()) {
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
        // A stale local credential on the current committed pane is not an
        // ordinary Register request. The daemon can prove the exact incumbent
        // from its own worker and binding records before admission; return the
        // explicit approval path without minting, persisting, or dispatching
        // Register.
        if approved.is_none() {
            match self.unapproved_stale_identity(&context, &request, &facts) {
                Ok(Some(result)) => return (self.host.clone(), typed_operation_resp(result)),
                Ok(None) => {}
                Err(error) => return (self.host.clone(), Resp::err(format!("{error:#}"))),
            }
        }
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
        };
        let approved_identity = approved.as_ref().map(|approved| approved.identity.clone());
        match self.reconcile_identity_context(
            context.clone(),
            facts,
            Some(outer_binding.clone()),
            approved_identity,
        ) {
            Ok((runtime, response, nested)) if response.ok => {
                let bound = outer_binding.bound();
                let nested_ref = nested.as_ref().or(bound.as_ref());
                // Business receipts are projected only from durable owner
                // evidence, in the accepted public order: approval decision,
                // nested Register, route, credential, optional grant, lease,
                // then the terminal `context_complete`. The helper stops at the
                // first owner whose durable proof is missing.
                let mut business_receipts = business_receipts_for_success(
                    &self.host,
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
                if let Some(approved) = approved.as_ref() {
                    // The distinct grant owner commits and reads back its own
                    // effect only when a separate grant approval was supplied.
                    // Identity-only recovery keeps the existing authority owner
                    // and never promotes, clears, or replaces it.
                    if approved.requires_grant_replacement() {
                        if let Err(reason) = self
                            .replace_approved_master_grant(&context, &request, &response, approved)
                        {
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
                // in the accepted order; only a committed explicit replacement
                // produces it.
                if grant_committed {
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
                        let _ = error;
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
                self.operation_journal.remove_live(&request.operation_id);
                let bound = outer_binding.bound();
                // The prepared envelope's IDs are durable but consume produced
                // no receipt (for example the existing CAS rejected a stale
                // revision). The outer projection must stay at the durable
                // `Validating` intent: `unknown`, never `inner_dispatched`
                // or `partial`, and no reprepare/retry.
                if outer_binding.bound().is_some() && nested.is_none() {
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
        validate_facts(&facts).map_err(PreflightIdentityError::Error)?;
        let scope = Scope { root };
        if !required_fields(&facts).is_empty() {
            self.complete_persisted_native_facts(&scope, &mut facts)
                .map_err(PreflightIdentityError::Error)?;
        }
        let required = required_fields(&facts);
        if !required.is_empty() {
            let descriptions = json!({
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

    /// Classify the narrow stale-cache case before any admission or Register.
    ///
    /// This is not a second credential authority. The resolver first proves
    /// the current pane maps to exactly one committed binding. The daemon's
    /// worker record then proves which token is current. Only when both facts
    /// are present, the persisted local cache names the same worker, and the
    /// cached token differs from the daemon record do we return the approval
    /// terminal. Any ambiguity or mismatch in the authoritative facts returns
    /// no classification and leaves the existing conflict path in charge.
    fn unapproved_stale_identity(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
        facts: &IdentityFacts,
    ) -> anyhow::Result<Option<crate::proto::ContextOperationResult>> {
        let Some(candidate) = facts.tmux.as_ref() else {
            return Ok(None);
        };
        let Ok(route) = self.resolve_route_by_tmux_endpoint(&candidate.endpoint) else {
            // Keep the existing typed conflict path for an absent, stale, or
            // ambiguous anchor. This preflight only classifies a proven one.
            return Ok(None);
        };
        if route.validate().is_err() {
            return Ok(None);
        }
        if route.project_scope != context.project_scope
            || route.app_scope_id != context.app_scope_id
            || route.canonical_root != context.canonical_root
        {
            return Ok(None);
        }
        let Some(persisted) =
            identity::read_persisted(&self.host.host_paths, route.agent_id.as_str())?
        else {
            return Ok(None);
        };
        if persisted.worker_id != route.agent_id.as_str()
            || persisted.project_scope.as_ref() != Some(&route.project_scope)
        {
            return Ok(None);
        }
        {
            let state = self.host.state.lock().unwrap();
            let Some(worker) = state.workers.get(route.agent_id.as_str()) else {
                return Ok(None);
            };
            if worker.token == persisted.token {
                return Ok(None);
            }
            let Some(binding) = state.global.lookup_binding_for(
                &crate::scope::RouteScope {
                    app_scope_id: route.app_scope_id.clone(),
                    project_scope_id: route.project_scope.clone(),
                },
                &route.binding_id,
            ) else {
                return Ok(None);
            };
            if binding.agent_id != route.agent_id
                || binding.endpoint_generation != route.endpoint_generation
                || binding.project_scope != route.project_scope
                || binding.app_scope_id != route.app_scope_id
            {
                return Ok(None);
            }
        }
        let mut approval = serde_json::json!({
            "decision": "approved",
            "target_identity": route.agent_id.as_str(),
            "project_scope": route.project_scope.as_str(),
            "app_scope_id": route.app_scope_id.as_str(),
            "action": "replace_binding",
            "expected_incumbent": {
                "binding_id": route.binding_id.as_str(),
                "endpoint_generation": route.endpoint_generation,
            }
        });
        approval["intent_digest"] =
            serde_json::Value::String(approval_digest(&approval, &request.facts));
        let mut result = operation_result(
            request,
            crate::proto::IdentityOperationProjection {
                operation_id: request.operation_id.clone(),
                phase: crate::proto::IdentityOperationPhase::Admitted,
                outcome: "denied".into(),
                committed_phases: Vec::new(),
                business_receipts: Vec::new(),
                nested_command_id: None,
                nested_operation_id: None,
            },
            false,
            None,
        );
        result.phase = None;
        result.requires.kind = Some("approval".into());
        result.requires.approval = Some(approval);
        result.requires.repair_invocation = Some(
            "collab context --approve-identity '<exact approval object from requires.approval>'"
                .into(),
        );
        result.owner_readback.insert(
            "identity".into(),
            serde_json::json!({
                "target_identity": route.agent_id.as_str(),
                "project_scope": route.project_scope.as_str(),
                "app_scope_id": route.app_scope_id.as_str(),
                "incumbent": {
                    "binding_id": route.binding_id.as_str(),
                    "endpoint_generation": route.endpoint_generation,
                },
                "reason": "IDENTITY_APPROVAL_REQUIRED",
            }),
        );
        Ok(Some(result))
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
        if operation.nested_command_id.is_none() {
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
            "automatic" | "supplement" | "approved_recovery"
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
