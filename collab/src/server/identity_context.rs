use super::*;
use crate::identity::{self, RuntimeIdentity};
use crate::proto::{AppServerCandidate, IdentityContextRequest, IdentityFacts};
use sha2::{Digest, Sha256};

/// Error from identity reconciliation that carries the nested Register receipt
/// IDs once that owner transaction has committed. The outer projection uses
/// the IDs to retain committed phases instead of reporting a bare pre-commit
/// `failed` for a post-commit error.
#[derive(Debug)]
struct ReconcileError {
    nested: Option<(String, String)>,
    error: anyhow::Error,
}

impl ReconcileError {
    fn before_register(error: anyhow::Error) -> Self {
        Self {
            nested: None,
            error,
        }
    }
}

impl std::fmt::Display for ReconcileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:#}", self.error)
    }
}

impl From<anyhow::Error> for ReconcileError {
    fn from(error: anyhow::Error) -> Self {
        Self::before_register(error)
    }
}

impl ProjectRuntimeManager {
    /// One host-local owner for bootstrap, recovery, registration and receipt.
    /// Register retains its own transaction/gate; never acquire it here.
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

    fn approved_recovery_admission(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
    ) -> Result<Option<ApprovedAdmission>, ApprovedAdmissionError> {
        let Some(raw_identity) = request.approval.as_ref() else {
            if request.grant_approval.is_some() {
                return Err(ApprovedAdmissionError::Error(
                    "IDENTITY_APPROVAL_REQUIRED: grant approval cannot replace the incumbent identity approval".into(),
                ));
            }
            return Ok(None);
        };
        let identity_approval =
            parse_identity_approval(raw_identity).map_err(ApprovedAdmissionError::Error)?;
        if identity_approval.project_scope != context.project_scope.as_str()
            || identity_approval.app_scope_id != context.app_scope_id.as_str()
        {
            return Err(ApprovedAdmissionError::Error(
                "IDENTITY_APPROVAL_SCOPE_MISMATCH".into(),
            ));
        }
        let digest = approval_digest(raw_identity, &request.facts);
        if identity_approval.intent_digest != digest {
            return Err(ApprovedAdmissionError::Error(
                "IDENTITY_APPROVAL_INTENT_MISMATCH".into(),
            ));
        }
        if identity_approval.decided_by.trim().is_empty() || identity_approval.approved_at_ms <= 0 {
            return Err(ApprovedAdmissionError::Error(
                "IDENTITY_APPROVAL_METADATA_INVALID".into(),
            ));
        }
        identity::validate_id_for_protocol(&identity_approval.target_identity).map_err(
            |error| {
                ApprovedAdmissionError::Error(format!("IDENTITY_APPROVAL_TARGET_INVALID: {error}"))
            },
        )?;
        let identity =
            identity::read_persisted(&self.host.host_paths, &identity_approval.target_identity)
                .map_err(|error| {
                    ApprovedAdmissionError::Error(format!(
                        "IDENTITY_APPROVAL_IDENTITY_READ_FAILED: {error:#}"
                    ))
                })?
                .ok_or_else(|| {
                    ApprovedAdmissionError::Error("IDENTITY_APPROVAL_IDENTITY_NOT_FOUND".into())
                })?;
        let incumbent = identity.runtime.as_ref().ok_or_else(|| {
            ApprovedAdmissionError::Error("IDENTITY_APPROVAL_INCUMBENT_NOT_REGISTERED".into())
        })?;
        let route_scope = crate::scope::RouteScope {
            app_scope_id: context.app_scope_id.clone(),
            project_scope_id: context.project_scope.clone(),
        };
        let observed_binding = {
            let state = self.host.state.lock().unwrap();
            state
                .global
                .lookup_binding_for(&route_scope, &incumbent.binding_id)
                .filter(|binding| binding.agent_id.as_str() == identity.worker_id)
                .cloned()
        };
        if incumbent.binding_id.as_str() != identity_approval.expected_incumbent_binding_id
            || incumbent.endpoint_generation != identity_approval.expected_incumbent_generation
            || observed_binding.as_ref().is_none_or(|binding| {
                binding.endpoint_generation != identity_approval.expected_incumbent_generation
            })
        {
            return Err(ApprovedAdmissionError::Denied(stale_approval_denial(
                request,
                &identity_approval,
                observed_binding.as_ref(),
            )));
        }

        // Parse and validate the distinct grant fence before admission. The
        // existing Register owner retains/rebinds a current grant atomically;
        // a grant-only replacement is not an identity recovery operation.
        let grant_before = {
            let state = self.host.state.lock().unwrap();
            current_master_grant(&state, Some(&route_scope))
        };
        let grant_approval = request
            .grant_approval
            .as_ref()
            .map(|raw| {
                let approval = parse_grant_approval(raw).map_err(ApprovedAdmissionError::Error)?;
                if approval.project_scope != context.project_scope.as_str()
                    || approval.app_scope_id != context.app_scope_id.as_str()
                    || approval.target_identity != identity.worker_id
                    || approval.intent_digest != approval_digest(raw, &request.facts)
                    || approval.decided_by.trim().is_empty()
                    || approval.approved_at_ms <= 0
                {
                    return Err(ApprovedAdmissionError::Error(
                        "GRANT_APPROVAL_SCOPE_OR_INTENT_MISMATCH".into(),
                    ));
                }
                let current = grant_before.as_ref().is_some_and(|grant| {
                    grant.agent_id.as_str() == identity.worker_id
                        && grant.resource_id() == approval.expected_grant_id
                        && grant.resource_generation() == approval.expected_grant_generation
                });
                if !current {
                    return Err(ApprovedAdmissionError::Error(
                        "GRANT_APPROVAL_STALE_FENCE".into(),
                    ));
                }
                Ok(approval)
            })
            .transpose()?;

        Ok(Some(ApprovedAdmission {
            identity_approval,
            grant_approval,
            grant_before,
            identity,
        }))
    }

    /// Commit the separately approved master-grant replacement under the
    /// existing authority owner, then read the committed grant back. The
    /// replacement reuses the current owner effect (scoped revoke + grant) and
    /// targets exactly the binding the Register owner just committed, so the
    /// authority owner, not the identity owner, owns this transition.
    fn replace_approved_master_grant(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
        response: &Resp,
        approved: &ApprovedAdmission,
    ) -> Result<(), String> {
        let approval = approved
            .grant_approval
            .as_ref()
            .ok_or_else(|| "grant replacement requested without an approval".to_string())?;
        let before = approved
            .grant_before
            .as_ref()
            .ok_or_else(|| "GRANT_APPROVAL_STALE_FENCE".to_string())?;
        let (binding_id, generation) = registered_binding_fence(response).ok_or_else(|| {
            "registered identity receipt has no runtime binding fence".to_string()
        })?;
        let route_scope = crate::scope::RouteScope {
            app_scope_id: context.app_scope_id.clone(),
            project_scope_id: context.project_scope.clone(),
        };
        let mut state = self.host.state.lock().unwrap();
        // The identity owner has already committed the new binding; the grant
        // owner rechecks that the binding it is about to fence is the current
        // one and still belongs to the approved target. The Register owner
        // reissues a same-principal grant for the new generation inside its own
        // transaction, so the grant fence must have advanced by exactly one
        // generation from the approved incumbent and kept the same grant id.
        let current_binding = state
            .global
            .lookup_binding_for(&route_scope, &binding_id)
            .cloned()
            .ok_or_else(|| "GRANT_APPROVAL_STALE_FENCE".to_string())?;
        if current_binding.agent_id.as_str() != approval.target_identity
            || current_binding.endpoint_generation != generation
        {
            return Err("GRANT_APPROVAL_STALE_FENCE".into());
        }
        // The Register owner already reissued a same-principal grant for the
        // new endpoint fence inside its own transaction. It preserves the
        // stable grant resource id and resource version, so the authority
        // owner only advances the resource version once here.
        let registered_grant = current_master_grant(&state, Some(&route_scope))
            .filter(|grant| {
                grant.agent_id.as_str() == approval.target_identity
                    && grant.resource_id() == before.resource_id()
                    && grant.resource_generation() == before.resource_generation()
                    && grant.binding_id.as_str() == binding_id.as_str()
                    && grant.endpoint_generation == generation
            })
            .ok_or_else(|| "GRANT_APPROVAL_STALE_FENCE".to_string())?;
        let expected_generation = registered_grant
            .resource_generation()
            .checked_add(1)
            .ok_or_else(|| "GRANT_APPROVAL_STALE_FENCE".to_string())?;
        let replacement = crate::server::global_state::MasterGrant::with_resource(
            route_scope.project_scope_id.clone(),
            route_scope.app_scope_id.clone(),
            current_binding.agent_id.clone(),
            registered_grant.boundary.clone(),
            approval.decided_by.clone(),
            grant_replacement_approval(approval),
            approval.expected_grant_id.clone(),
            expected_generation,
            binding_id.clone(),
            generation,
            now_ms(),
        )
        .map_err(|error| error.to_string())?;
        #[cfg(feature = "context-cancel-test-hooks")]
        if crate::context_cancel_test_hooks::barrier_reply(
            "before_grant_owner",
            &request.operation_id,
            std::process::id(),
            None,
            None,
            || false,
        )
        .as_deref()
            == Some("fail_owner")
        {
            return Err("injected grant owner failure before Start".into());
        }
        // Arbitration with cancellation: mark the grant owner active before the
        // durable Start append. A cancellation that already won is refused and
        // the caller returns the durable cancelled projection.
        self.operation_journal
            .begin_owner_start(&request.operation_id, "grant")?;
        let intent_id = format!("grant-replacement-{}", request.operation_id);
        let intent = crate::server::global_state::MasterGrantReplacementIntent {
            operation_id: request.operation_id.clone(),
            intent_id: intent_id.clone(),
            project_scope: route_scope.project_scope_id.clone(),
            app_scope_id: route_scope.app_scope_id.clone(),
            target_identity: current_binding.agent_id.clone(),
            incumbent_grant_id: before.resource_id().to_owned(),
            incumbent_grant_generation: before.resource_generation(),
            binding_id: binding_id.clone(),
            endpoint_generation: generation,
            expected_grant_id: approval.expected_grant_id.clone(),
            expected_grant_generation: expected_generation,
            approval_digest: approval.intent_digest.clone(),
            started_at_ms: now_ms(),
        };
        let start_event =
            crate::server::state::Event::GlobalMasterGrantReplacementStarted { intent };
        if let Err(error) = self
            .host
            .commit_locked(&mut state, std::slice::from_ref(&start_event))
        {
            return Err(format!("MASTER_DURABILITY_FAILED: {error}"));
        }
        #[cfg(feature = "context-cancel-test-hooks")]
        crate::context_cancel_test_hooks::barrier(
            "grant_start_synced",
            &request.operation_id,
            std::process::id(),
            None,
            None,
            || false,
        );
        let mut events = master_authority_transfer_events(&state, &route_scope, replacement);
        events.push(
            crate::server::state::Event::GlobalMasterGrantReplacementCompleted {
                receipt: crate::server::global_state::MasterGrantReplacementReceipt {
                    operation_id: request.operation_id.clone(),
                    intent_id,
                    project_scope: route_scope.project_scope_id.clone(),
                    app_scope_id: route_scope.app_scope_id.clone(),
                    grant_id: approval.expected_grant_id.clone(),
                    grant_generation: expected_generation,
                    binding_id: binding_id.clone(),
                    endpoint_generation: generation,
                    completed_at_ms: now_ms(),
                },
            },
        );
        if let Err(error) = self.host.commit_locked(&mut state, &events) {
            return Err(format!("MASTER_DURABILITY_FAILED: {error}"));
        }
        let committed = current_master_grant(&state, Some(&route_scope));
        drop(state);
        let matches = committed.is_some_and(|grant| {
            grant.agent_id.as_str() == approval.target_identity
                && grant.resource_id() == approval.expected_grant_id
                && grant.resource_generation() == expected_generation
                && grant.binding_id.as_str() == binding_id.as_str()
                && grant.endpoint_generation == generation
                && grant.approval == grant_replacement_approval(approval)
        });
        matches
            .then_some(())
            .ok_or_else(|| "grant readback mismatch".to_string())
    }

    /// Identity-only recovery must leave the existing authority owner in
    /// place. The Register owner already reissues a same-principal grant for
    /// the new generation inside its own transaction, so the readback proves
    /// the grant still names this identity and binding; the generation is
    /// expected to advance with the recovered binding.
    fn identity_only_grant_retained(
        &self,
        context: &ProjectContext,
        response: &Resp,
        approved: &ApprovedAdmission,
    ) -> bool {
        let Some(before) = approved.grant_before.as_ref() else {
            return true;
        };
        let Some((binding_id, generation)) = registered_binding_fence(response) else {
            return false;
        };
        let state = self.host.state.lock().unwrap();
        current_master_grant(
            &state,
            Some(&crate::scope::RouteScope {
                app_scope_id: context.app_scope_id.clone(),
                project_scope_id: context.project_scope.clone(),
            }),
        )
        .is_some_and(|after| {
            after.agent_id == before.agent_id
                && after.resource_id() == before.resource_id()
                && after.resource_generation() == before.resource_generation()
                && after.binding_id.as_str() == binding_id.as_str()
                && after.endpoint_generation == generation
        })
    }

    fn reconcile_identity_context(
        &self,
        context: ProjectContext,
        mut facts: IdentityFacts,
        outer_binding: Option<RegisterOuterBinding>,
        approved_identity: Option<identity::Identity>,
    ) -> Result<(Arc<Server>, Resp, Option<(String, String)>), ReconcileError> {
        if context.app_scope_id.as_str() != identity::CLI_APP_SERVER_ID
            || context.runtime_context.is_some()
        {
            return Err(ReconcileError::before_register(anyhow::anyhow!(
                "IDENTITY_CONTEXT_INVALID: bootstrap requires the CLI project scope without a caller-selected identity"
            )));
        }
        let root = PathBuf::from(&context.canonical_root);
        context.validate_registered_root(&root)?;
        validate_project_registration_cwd(&context.canonical_root, &root)
            .map_err(anyhow::Error::msg)?;
        validate_facts(&facts)?;
        let scope = Scope { root };
        if !required_fields(&facts).is_empty() {
            self.complete_persisted_native_facts(&scope, &mut facts)?;
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
            return Ok((
                self.host.clone(),
                Resp::data(json!({
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
                })),
                None,
            ));
        }
        let pane_route = if let Some(candidate) = facts.tmux.as_ref() {
            match self.resolve_route_by_tmux_endpoint(&candidate.endpoint) {
                Ok(route) => Some(route),
                // A stale host index, or no route at all, is not usable route
                // evidence: the caller still bootstraps from its live anchors.
                // Every other error stays fatal, so an ambiguity or a real
                // binding conflict still reaches the identity owner.
                Err(error)
                    if error.starts_with("ROUTE_RESOLVE_NOT_FOUND:")
                        || error.starts_with("ROUTE_RESOLVE_STALE_INDEX:") =>
                {
                    None
                }
                Err(error) => return Err(anyhow::Error::msg(error).into()),
            }
        } else {
            None
        };
        let mut ident = if let Some(identity) = approved_identity {
            identity
        } else {
            let mut identity = identity::resolve_for_daemon_with_route_at(
                &self.host.host_paths,
                &scope,
                &facts,
                pane_route.as_ref(),
            )?;
            self.reconcile_committed_credential(&scope, &facts, &mut identity)?;
            identity
        };
        let candidates = TransportCandidates {
            appserver: facts.endpoint.as_ref().map(|endpoint| AppServerCandidate {
                endpoint: endpoint.clone(),
                namespace: facts.namespace.clone().expect("complete native facts"),
                session_id: facts.session_id.clone().expect("complete native facts"),
                thread_id: facts.thread_id.clone().expect("complete native facts"),
                cwd: context.canonical_root.clone(),
            }),
            tmux: facts.tmux,
            dsh: None,
        };
        let provisional = RuntimeIdentity::cli_adapter(&ident.worker_id)?;
        let register_runtime = ident.runtime.as_ref().unwrap_or(&provisional);
        let register_context = ProjectContext::for_registered_route(&scope.root, register_runtime)?;
        let (runtime, receipt) = self.dispatch_sync_with_register_binding(
            Some(register_context),
            Req::Register {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
                cwd: context.canonical_root,
                candidates: Some(candidates),
            },
            outer_binding.as_ref(),
        );
        if !receipt.ok {
            return Ok((runtime, receipt, None));
        }
        let nested = (
            receipt
                .data
                .get("command_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    ReconcileError::before_register(anyhow::anyhow!(
                        "IDENTITY_CONTEXT_INVALID: register receipt has no command_id"
                    ))
                })?
                .to_owned(),
            receipt
                .data
                .get("operation_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    ReconcileError::before_register(anyhow::anyhow!(
                        "IDENTITY_CONTEXT_INVALID: register receipt has no operation_id"
                    ))
                })?
                .to_owned(),
        );
        let post_register = |error: anyhow::Error| ReconcileError {
            nested: Some(nested.clone()),
            error,
        };
        let (binding, transport) =
            identity::registration_from_receipt(&receipt.data, &ident.worker_id, &scope.root)
                .map_err(post_register)?;
        identity::persist_registration_at(
            &self.host.host_paths,
            &scope,
            &mut ident,
            binding.clone(),
            transport,
        )
        .map_err(post_register)?;
        let registered_context =
            ProjectContext::for_registered_route(&scope.root, &binding).map_err(post_register)?;
        let (runtime, snapshot) = self.dispatch_sync(
            Some(registered_context),
            Req::Context {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
            },
        );
        if !snapshot.ok {
            return Ok((runtime, snapshot, Some(nested)));
        }
        Ok((
            runtime,
            Resp::data(json!({
                "snapshot": snapshot.data,
                "identity_receipt": ident
            })),
            Some(nested),
        ))
    }

    /// A successful supplement belongs to its committed identity. A unique
    /// current anchor may reuse that typed receipt on later partial calls;
    /// anonymous callers and conflicting observations never inherit it.
    fn complete_persisted_native_facts(
        &self,
        scope: &Scope,
        facts: &mut IdentityFacts,
    ) -> anyhow::Result<()> {
        let Some(existing) = identity::existing_for_daemon_at(&self.host.host_paths, scope, facts)?
        else {
            return Ok(());
        };
        let (Some(runtime), Some(transport)) = (existing.runtime, existing.transport) else {
            return Ok(());
        };
        // A dsh identity never reaches here: with no App Server endpoint observed,
        // `required_fields` returns empty and this function is not called. Its
        // gateway-owned address is not an App Server supplement the caller can
        // provide; without a gateway the caller fails at `TRANSPORT_NONE`.
        if transport.kind == TransportKind::Dsh {
            return Ok(());
        }
        if transport.kind != TransportKind::AppServer
            || facts.session_id.as_deref().is_some_and(|value| {
                runtime.session_id.as_ref().map(identity::SessionId::as_str) != Some(value)
            })
            || facts.thread_id.as_deref().is_some_and(|value| {
                runtime
                    .native_thread_id
                    .as_ref()
                    .map(NativeThreadId::as_str)
                    != Some(value)
            })
        {
            return Ok(());
        }
        facts.session_id = facts
            .session_id
            .take()
            .or_else(|| runtime.session_id.map(|value| value.to_string()));
        facts.thread_id = facts
            .thread_id
            .take()
            .or_else(|| runtime.native_thread_id.map(|value| value.to_string()));
        let address_matches = facts
            .endpoint
            .as_ref()
            .is_none_or(|value| Some(value) == transport.endpoint.as_ref())
            && facts
                .namespace
                .as_ref()
                .is_none_or(|value| Some(value) == transport.namespace.as_ref());
        if address_matches {
            facts.endpoint = facts.endpoint.take().or(transport.endpoint);
            facts.namespace = facts.namespace.take().or(transport.namespace);
        }
        Ok(())
    }

    /// A committed registration can outlive its local receipt file. Recover
    /// that same credential from the reducer only for the proven anchor.
    /// A committed persisted credential is never replaced, even when it is
    /// rejected. A runtime-less draft is not a credential: the only production
    /// writer (`persist_registration_at`) always persists `runtime` together
    /// with `transport`, so a file without `runtime` is legacy residue and must
    /// not mask the committed record for the same anchor.
    fn reconcile_committed_credential(
        &self,
        scope: &Scope,
        facts: &IdentityFacts,
        ident: &mut identity::Identity,
    ) -> anyhow::Result<()> {
        let committed_locally = identity::read_persisted(&self.host.host_paths, &ident.worker_id)?
            .is_some_and(|persisted| persisted.runtime.is_some());
        if ident.runtime.is_some() || committed_locally {
            return Ok(());
        }
        let host_binding = {
            let state = self.host.state.lock().unwrap();
            if let Some(tmux) = &facts.tmux {
                state
                    .global
                    .lookup_unique_tmux_pane_route(&tmux.endpoint)
                    .cloned()
            } else if let (Some(session), Some(thread)) = (&facts.session_id, &facts.thread_id) {
                state
                    .global
                    .lookup_current_thread_route(
                        &identity::SessionId::new(session.clone())?,
                        &NativeThreadId::new(thread.clone())?,
                    )
                    .cloned()
            } else {
                None
            }
        };
        if let Some(binding) = host_binding {
            if binding.project_scope.as_str() == scope.root.to_string_lossy()
                && binding.agent_id.as_str() == ident.worker_id
            {
                let context = ProjectContext::for_registered_root_with_app(
                    &scope.root,
                    binding.app_scope_id,
                )?;
                self.select_runtime(&context).map_err(anyhow::Error::msg)?;
            }
        }
        let mut recovered = None;
        for runtime in self.runtimes() {
            let state = runtime.state.lock().unwrap();
            let Some(project) = state.global.lookup_project(&ProjectScopeId::new(
                scope.root.to_string_lossy().into_owned(),
            )?) else {
                continue;
            };
            for binding in project.runtime_bindings.values() {
                let anchor_matches = if let Some(tmux) = &facts.tmux {
                    binding.tmux_endpoint.as_ref().is_some_and(|bound| {
                        // Ownership follows the pane address, never the concrete
                        // endpoint. A reissued shell pid in the same owned pane
                        // is still this anchor, so this must agree with the
                        // resolver's `same_owned_pane` arm.
                        crate::client::adapters::tmux::same_owned_pane(bound, &tmux.endpoint)
                    })
                } else {
                    binding.session_id.as_ref().map(identity::SessionId::as_str)
                        == facts.session_id.as_deref()
                        && binding
                            .native_thread_id
                            .as_ref()
                            .map(NativeThreadId::as_str)
                            == facts.thread_id.as_deref()
                };
                if binding.agent_id.as_str() != ident.worker_id || !anchor_matches {
                    continue;
                }
                let Some(worker) = state.workers.get(&ident.worker_id) else {
                    continue;
                };
                let Some(transport) = worker.transport.clone() else {
                    anyhow::bail!("IDENTITY_RESTORE_CONFLICT: committed worker has no transport");
                };
                let candidate = identity::Identity {
                    worker_id: ident.worker_id.clone(),
                    token: worker.token.clone(),
                    project_scope: Some(binding.project_scope.clone()),
                    runtime: Some(RuntimeIdentity {
                        agent_id: binding.agent_id.clone(),
                        runtime_id: binding.runtime_id.clone(),
                        appserver_id: binding.app_scope_id.clone(),
                        endpoint_generation: binding.endpoint_generation,
                        binding_id: binding.binding_id.clone(),
                        session_id: binding.session_id.clone(),
                        native_thread_id: binding.native_thread_id.clone(),
                    }),
                    transport: Some(transport),
                };
                if recovered
                    .as_ref()
                    .is_some_and(|prior: &identity::Identity| {
                        prior.token != candidate.token || prior.runtime != candidate.runtime
                    })
                {
                    anyhow::bail!("IDENTITY_RESTORE_CONFLICT: current anchor has conflicting committed credentials");
                }
                recovered = Some(candidate);
            }
        }
        if let Some(recovered) = recovered {
            *ident = recovered;
        }
        Ok(())
    }
}

fn validate_facts(facts: &IdentityFacts) -> anyhow::Result<()> {
    for (name, value) in [
        ("session_id", facts.session_id.as_deref()),
        ("thread_id", facts.thread_id.as_deref()),
        ("endpoint", facts.endpoint.as_deref()),
        ("namespace", facts.namespace.as_deref()),
    ] {
        if let Some(value) = value {
            if value.trim().is_empty() {
                anyhow::bail!("IDENTITY_FACT_INVALID: {name} must not be empty");
            }
            if matches!(name, "session_id" | "thread_id") {
                identity::validate_id_for_protocol(value)?;
            } else if value.chars().any(char::is_control) {
                anyhow::bail!("IDENTITY_FACT_INVALID: {name} contains control characters");
            }
        }
    }
    if let Some(namespace) = facts.namespace.as_deref() {
        if !matches!(namespace, "codex_tui" | "codex_app") {
            anyhow::bail!("IDENTITY_FACT_INVALID: unsupported namespace {namespace}");
        }
    }
    if let Some(endpoint) = facts.endpoint.as_deref() {
        let path = endpoint
            .strip_prefix("unix://")
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_FACT_INVALID: endpoint must be unix://"))?;
        if !Path::new(path).is_absolute() {
            anyhow::bail!("IDENTITY_FACT_INVALID: endpoint must contain an absolute socket path");
        }
    }
    Ok(())
}

fn identity_operation_intent_digest(
    context: &ProjectContext,
    request: &IdentityContextRequest,
) -> String {
    let normalized = json!({
        "project_scope": context.project_scope.as_str(),
        "app_scope_id": context.app_scope_id.as_str(),
        "target_identity": null,
        "action": request.action,
        "invocation": request.invocation,
        "facts": request.facts,
        "approval": request.approval,
        "grant_approval": request.grant_approval,
    });
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&normalized).unwrap_or_default());
    format!("sha256:{:x}", digest.finalize())
}

/// Build a `Resp` for a typed operation outcome.
///
/// `Resp.ok` owns transport/operation success and the flattened payload is the
/// single `result` object. An inner `ok` would collide with `Resp.ok` on the
/// flattened wire and deserialize as duplicate keys, so the typed outcome is
/// carried only under `result`. A typed incomplete result is `ok: false` so the
/// CLI exit/MCP `isError` projection can distinguish it from transport failure.
fn typed_operation_resp(result: crate::proto::ContextOperationResult) -> Resp {
    let ok = result.outcome == "completed";
    let data = serde_json::json!({
        "result": serde_json::to_value(result).unwrap_or(serde_json::Value::Null)
    });
    Resp {
        ok,
        error: None,
        data,
    }
}

fn operation_result(
    request: &IdentityContextRequest,
    projection: crate::proto::IdentityOperationProjection,
    ok: bool,
    snapshot: Option<serde_json::Value>,
) -> crate::proto::ContextOperationResult {
    let phase = projection
        .business_receipts
        .last()
        .cloned()
        .or_else(|| ok.then(|| "context_complete".to_owned()));
    crate::proto::ContextOperationResult {
        operation_id: projection.operation_id,
        invocation: request.invocation.clone(),
        action: request.action.clone(),
        phase,
        outcome: projection.outcome,
        committed_phases: projection.business_receipts,
        failed_phase: None,
        requires: crate::proto::ContextOperationRequires {
            kind: (!ok).then(|| "operation".to_owned()),
            fields: Vec::new(),
            sources: Default::default(),
            approval: None,
            repair_invocation: None,
        },
        snapshot,
        owner_readback: Default::default(),
        queried_operation: None,
    }
}

fn operation_refusal(
    request: &IdentityContextRequest,
    error: String,
    repair_invocation: &str,
) -> crate::proto::ContextOperationResult {
    let mut result = operation_result(
        request,
        crate::proto::IdentityOperationProjection {
            operation_id: request.operation_id.clone(),
            phase: crate::proto::IdentityOperationPhase::Refused,
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
    result
        .owner_readback
        .insert("error".into(), serde_json::Value::String(error));
    result.requires.repair_invocation = Some(repair_invocation.to_owned());
    result
}

fn stale_approval_denial(
    request: &IdentityContextRequest,
    approval: &IdentityApproval,
    observed: Option<&crate::server::global_state::RuntimeBinding>,
) -> crate::proto::ContextOperationResult {
    let mut result = operation_refusal(
        request,
        "APPROVAL_STALE_CONFLICT".into(),
        "collab context --op <new-operation-id> --approve-identity '<fresh approval object>'",
    );
    result.requires.kind = Some("approval".into());
    result.requires.repair_invocation = Some(
        "collab context --approve-identity '<exact approval object from requires.approval>'".into(),
    );
    result.owner_readback.insert(
        "error".into(),
        serde_json::Value::String("APPROVAL_STALE_CONFLICT".into()),
    );
    result.owner_readback.insert(
        "expected_incumbent".into(),
        serde_json::json!({
            "binding_id": approval.expected_incumbent_binding_id,
            "endpoint_generation": approval.expected_incumbent_generation,
        }),
    );
    if let Some(observed) = observed {
        let mut fresh_approval = serde_json::json!({
            "decision": "approved",
            "target_identity": observed.agent_id.as_str(),
            "project_scope": observed.project_scope.as_str(),
            "app_scope_id": observed.app_scope_id.as_str(),
            "action": "replace_binding",
            "expected_incumbent": {
                "binding_id": observed.binding_id.as_str(),
                "endpoint_generation": observed.endpoint_generation,
            }
        });
        fresh_approval["intent_digest"] =
            serde_json::Value::String(approval_digest(&fresh_approval, &request.facts));
        result.requires.approval = Some(fresh_approval);
        result.owner_readback.insert(
            "observed_incumbent".into(),
            serde_json::json!({
                "binding_id": observed.binding_id.as_str(),
                "endpoint_generation": observed.endpoint_generation,
                "target_identity": observed.agent_id.as_str(),
                "project_scope": observed.project_scope.as_str(),
                "app_scope_id": observed.app_scope_id.as_str(),
            }),
        );
    }
    result
}

fn missing_facts_result(
    request: &IdentityContextRequest,
    data: &serde_json::Value,
) -> crate::proto::ContextOperationResult {
    let update = data
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("requires_identity_update"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let fields: Vec<String> = update
        .get("required_fields")
        .and_then(serde_json::Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let sources = fields
        .iter()
        .map(|field| {
            (
                field.clone(),
                update
                    .get("field_descriptions")
                    .and_then(|value| value.get(field))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
    crate::proto::ContextOperationResult {
        operation_id: request.operation_id.clone(),
        invocation: request.invocation.clone(),
        action: request.action.clone(),
        phase: None,
        outcome: "missing_facts".into(),
        committed_phases: Vec::new(),
        failed_phase: None,
        requires: crate::proto::ContextOperationRequires {
            kind: Some("identity_facts".into()),
            fields,
            sources,
            approval: None,
            repair_invocation: update
                .get("action")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
        },
        snapshot: None,
        owner_readback: Default::default(),
        queried_operation: None,
    }
}

/// Which facts the caller must still supply before an identity can be resolved.
///
/// Only the App Server path has fields the caller may have to add: its
/// endpoint, namespace, session and thread are four separate observations that
/// can arrive incomplete. The tmux and dsh paths each present one designed
/// anchor that is complete on its own, so they require nothing further.
///
/// A tmux or dsh anchor only short-circuits the request while no App Server
/// endpoint was observed. If an endpoint *is* present, the four App Server
/// facts are still demanded: the endpoint selects the App Server candidate
/// below, which reads all four fields unconditionally.
fn required_fields(facts: &IdentityFacts) -> Vec<&'static str> {
    if (facts.tmux.is_some() || facts.dsh_session_id.is_some()) && facts.endpoint.is_none() {
        return Vec::new();
    }
    [
        ("session_id", &facts.session_id),
        ("thread_id", &facts.thread_id),
        ("endpoint", &facts.endpoint),
        ("namespace", &facts.namespace),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.is_none().then_some(name))
    .collect()
}

enum PreflightIdentityError {
    MissingFacts(serde_json::Value),
    Error(anyhow::Error),
}

/// A user approval parsed from the exact staged schema. It binds one target
/// identity, one route scope, one action, and one exact incumbent fence. The
/// daemon validates these fields; it never trusts a caller's identity claim.
#[derive(Debug, Clone)]
struct IdentityApproval {
    decision: String,
    decided_by: String,
    target_identity: String,
    project_scope: String,
    app_scope_id: String,
    action: String,
    expected_incumbent_binding_id: String,
    expected_incumbent_generation: u64,
    intent_digest: String,
    approved_at_ms: i64,
}

/// The separate owner fence for a master grant replacement. It is never
/// inferred from the identity approval.
#[derive(Debug, Clone)]
struct GrantApproval {
    decision: String,
    decided_by: String,
    target_identity: String,
    project_scope: String,
    app_scope_id: String,
    action: String,
    expected_grant_id: String,
    expected_grant_generation: u64,
    intent_digest: String,
    approved_at_ms: i64,
}

/// One admitted approved-recovery intent: the exact target identity the daemon
/// will Register, the validated incumbent fence, and the optional separate
/// grant fence.
struct ApprovedAdmission {
    identity_approval: IdentityApproval,
    grant_approval: Option<GrantApproval>,
    grant_before: Option<crate::server::global_state::MasterGrant>,
    identity: identity::Identity,
}

impl ApprovedAdmission {
    /// The daemon-issued proof that lets the Register owner admit an approved
    /// stale-credential replacement for exactly this target and scope.
    fn register_proof(&self, context: &ProjectContext) -> RegisterApprovalProof {
        RegisterApprovalProof {
            target_identity: self.identity.worker_id.clone(),
            project_scope: context.project_scope.as_str().to_owned(),
            app_scope_id: context.app_scope_id.as_str().to_owned(),
            incumbent_binding_id: self.identity_approval.expected_incumbent_binding_id.clone(),
            incumbent_endpoint_generation: self.identity_approval.expected_incumbent_generation,
        }
    }

    /// A grant approval is a distinct owner authorization: the identity owner
    /// must not treat it as a second identity approval, and the grant owner
    /// commits its replacement in its own transaction.
    fn requires_grant_replacement(&self) -> bool {
        self.grant_approval.is_some()
    }
}

enum ApprovedAdmissionError {
    /// A typed refusal returned to the caller without any side effect.
    Denied(crate::proto::ContextOperationResult),
    /// A malformed request or an internal invariant break.
    Error(String),
}

fn validate_operation_request(request: &IdentityContextRequest) -> Result<(), String> {
    // A malformed query shape (query flag or query invocation/action reaching
    // the mutation owner) is reported before the unsupported-invocation gate so
    // callers get the precise selector error instead of a generic refusal.
    if request.query {
        return Err("IDENTITY_OPERATION_QUERY_SHAPE_INVALID".into());
    }
    if request.invocation == "query" || request.action == "query" {
        return Err("IDENTITY_OPERATION_QUERY_SHAPE_INVALID".into());
    }
    if request.action != "context" {
        return Err("IDENTITY_OPERATION_INTENT_INVALID: action must be context".into());
    }
    let approved = request.approval.is_some() || request.grant_approval.is_some();
    if approved {
        if request.invocation != "approved_recovery" {
            return Err(
                "IDENTITY_OPERATION_INTENT_INVALID: approvals require the approved_recovery invocation"
                    .into(),
            );
        }
    } else {
        if request.invocation != "automatic" && request.invocation != "supplement" {
            return Err("IDENTITY_CONTEXT_UNSUPPORTED: this adapter slice supports automatic and supplement context only".into());
        }
    }
    if request.query_capability.trim().is_empty() {
        return Err(
            "IDENTITY_OPERATION_CAPABILITY_REQUIRED: mutating context requires a query capability"
                .into(),
        );
    }
    Ok(())
}

fn parse_identity_approval(value: &serde_json::Value) -> Result<IdentityApproval, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "approval must be a JSON object".to_string())?;
    let decision = object
        .get("decision")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if decision != "approved" {
        return Err(format!(
            "IDENTITY_APPROVAL_NOT_APPROVED: decision must be approved, observed {decision}"
        ));
    }
    let target_identity = required_approval_string(object, "target_identity")?;
    let project_scope = required_approval_string(object, "project_scope")?;
    let app_scope_id = required_approval_string(object, "app_scope_id")?;
    let action = required_approval_string(object, "action")?;
    if action != "replace_binding" && action != "restore_identity" {
        return Err(format!(
            "IDENTITY_APPROVAL_ACTION_INVALID: action must be replace_binding or restore_identity, observed {action}"
        ));
    }
    let incumbent = object
        .get("expected_incumbent")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            "IDENTITY_APPROVAL_FENCE_REQUIRED: expected_incumbent is required".to_string()
        })?;
    let expected_incumbent_binding_id = required_approval_string(incumbent, "binding_id")?;
    let expected_incumbent_generation = incumbent
        .get("endpoint_generation")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            "IDENTITY_APPROVAL_FENCE_REQUIRED: expected_incumbent.endpoint_generation is required"
                .to_string()
        })?;
    Ok(IdentityApproval {
        decision: decision.to_owned(),
        decided_by: required_approval_string(object, "decided_by")?,
        target_identity,
        project_scope,
        app_scope_id,
        action,
        expected_incumbent_binding_id,
        expected_incumbent_generation,
        intent_digest: required_approval_string(object, "intent_digest")?,
        approved_at_ms: object
            .get("approved_at_ms")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                "IDENTITY_APPROVAL_METADATA_INVALID: approved_at_ms is required".to_string()
            })?,
    })
}

fn parse_grant_approval(value: &serde_json::Value) -> Result<GrantApproval, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "grant_approval must be a JSON object".to_string())?;
    let decision = object
        .get("decision")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if decision != "approved" {
        return Err(format!(
            "GRANT_APPROVAL_NOT_APPROVED: decision must be approved, observed {decision}"
        ));
    }
    let target_identity = required_approval_string(object, "target_identity")?;
    let project_scope = required_approval_string(object, "project_scope")?;
    let app_scope_id = required_approval_string(object, "app_scope_id")?;
    let action = required_approval_string(object, "action")?;
    if action != "replace_master_grant" {
        return Err(format!(
            "GRANT_APPROVAL_ACTION_INVALID: action must be replace_master_grant, observed {action}"
        ));
    }
    let grant = object
        .get("expected_grant")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "GRANT_APPROVAL_FENCE_REQUIRED: expected_grant is required".to_string())?;
    let expected_grant_id = required_approval_string(grant, "grant_id")?;
    let expected_grant_generation = grant
        .get("generation")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            "GRANT_APPROVAL_FENCE_REQUIRED: expected_grant.generation is required".to_string()
        })?;
    Ok(GrantApproval {
        decision: decision.to_owned(),
        decided_by: required_approval_string(object, "decided_by")?,
        target_identity,
        project_scope,
        app_scope_id,
        action,
        expected_grant_id,
        expected_grant_generation,
        intent_digest: required_approval_string(object, "intent_digest")?,
        approved_at_ms: object
            .get("approved_at_ms")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                "GRANT_APPROVAL_METADATA_INVALID: approved_at_ms is required".to_string()
            })?,
    })
}

fn approval_digest(approval: &serde_json::Value, facts: &IdentityFacts) -> String {
    let mut approval = approval.clone();
    if let Some(object) = approval.as_object_mut() {
        object.remove("intent_digest");
        object.remove("decided_by");
        object.remove("approved_at_ms");
    }
    let normalized = json!({"approval": approval, "facts": facts});
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&normalized).unwrap_or_default());
    format!("sha256:{:x}", digest.finalize())
}

/// The exact runtime binding fence the Register owner just committed, read
/// from the owner's own identity receipt. Both the grant owner and the
/// identity-only retention check bind to this value, never to a guessed one.
fn registered_binding_fence(response: &Resp) -> Option<(BindingId, u64)> {
    let runtime = response
        .data
        .get("identity_receipt")
        .and_then(|value| value.get("runtime"))?;
    let binding_id = runtime
        .get("binding_id")
        .and_then(serde_json::Value::as_str)?;
    let generation = runtime
        .get("endpoint_generation")
        .and_then(serde_json::Value::as_u64)?;
    Some((BindingId::new(binding_id.to_owned()).ok()?, generation))
}

/// The durable approval text recorded on a separately approved replacement
/// grant. It carries the user's decision and approval time so the authority
/// ledger keeps the same authorization evidence the identity owner validated.
/// Ordered durable business receipts proved by the current owner state after a
/// successful reconciliation. The list stops at the first owner whose durable
/// result is missing, so a receipt is only present when its own owner persisted
/// the effect. It never includes `context_complete`; the caller appends that
/// terminal receipt only after the optional authority receipt and the lease
/// proof are both present.
fn business_receipts_for_success(
    host: &Server,
    context: &ProjectContext,
    approved: Option<&ApprovedAdmission>,
    response: &Resp,
    nested: Option<&(String, String)>,
) -> Vec<String> {
    let mut receipts = Vec::new();
    if approved.is_some() {
        receipts.push("approval_decision".to_owned());
    }
    let Some((command_id, operation_id)) = nested else {
        return receipts;
    };
    if command_id.trim().is_empty() || operation_id.trim().is_empty() {
        return receipts;
    }
    receipts.push("nested_register".to_owned());
    let Some(worker_id) = response
        .data
        .get("identity_receipt")
        .and_then(|receipt| receipt.get("worker_id"))
        .and_then(serde_json::Value::as_str)
    else {
        return receipts;
    };
    let Some((binding_id, generation)) = registered_binding_fence(response) else {
        return receipts;
    };
    let route_scope = crate::scope::RouteScope {
        app_scope_id: context.app_scope_id.clone(),
        project_scope_id: context.project_scope.clone(),
    };
    // Route owner: the committed route must name exactly the binding version
    // the Register receipt returned, for this exact scope and principal.
    let route_proven = {
        let state = host.state.lock().unwrap();
        state
            .global
            .lookup_binding_for(&route_scope, &binding_id)
            .is_some_and(|binding| {
                binding.agent_id.as_str() == worker_id && binding.endpoint_generation == generation
            })
    };
    if !route_proven {
        return receipts;
    }
    receipts.push("route".to_owned());
    // Credential owner: the durable persisted identity, not the in-memory
    // worker record, must carry the exact committed binding version.
    let credential_proven = identity::read_persisted(&host.host_paths, worker_id)
        .ok()
        .flatten()
        .and_then(|identity| identity.runtime)
        .is_some_and(|runtime| {
            runtime.binding_id.as_str() == binding_id.as_str()
                && runtime.endpoint_generation == generation
        });
    if !credential_proven {
        return receipts;
    }
    receipts.push("credential".to_owned());
    // Lease owner: the Register owner armed a default direct-message lease for
    // this worker. It must still be live, not merely recorded.
    let lease_proven = {
        let state = host.state.lock().unwrap();
        state
            .notification_subscriptions
            .get(&crate::server::mailbox::default_direct_message_id(
                worker_id,
            ))
            .is_some_and(|subscription| {
                subscription.worker_id == worker_id
                    && subscription.status == "armed"
                    && subscription.expires_ms > crate::server::state::now_ms()
            })
    };
    if lease_proven {
        receipts.push("lease".to_owned());
    }
    receipts
}

/// Receipts that remain proved when reconciliation cannot complete. Only the
/// identity admission decision and a committed nested Register receipt are
/// durable at this point; route, credential, lease and `context_complete` are
/// never fabricated from a lifecycle name.
fn business_receipts_for_failure(
    approved: Option<&ApprovedAdmission>,
    nested: Option<&(String, String)>,
) -> Vec<String> {
    let mut receipts = Vec::new();
    if approved.is_some() {
        receipts.push("approval_decision".to_owned());
    }
    if let Some((command_id, operation_id)) = nested {
        if !command_id.trim().is_empty() && !operation_id.trim().is_empty() {
            receipts.push("nested_register".to_owned());
        }
    }
    receipts
}

fn grant_replacement_approval(approval: &GrantApproval) -> String {
    format!(
        "{}:{}:{}",
        approval.decision, approval.decided_by, approval.approved_at_ms
    )
}

fn required_approval_string(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<String, String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            format!("IDENTITY_APPROVAL_FIELD_REQUIRED: {field} must be a non-empty string")
        })
}

#[cfg(test)]
mod required_fields_tests {
    use super::*;
    use crate::proto::TmuxCandidate;
    use crate::server::operation_journal::{inject_next_append_fault, OperationJournal};
    use crate::server::peer_tests::test_server;
    use std::sync::Arc;

    fn tmux_anchor() -> TmuxCandidate {
        TmuxCandidate {
            endpoint: crate::proto::TmuxEndpoint {
                socket_path: "/tmp/tmux-test.sock".into(),
                server_pid: 42,
                tmux_session_id: "$7".into(),
                pane_id: "%3".into(),
                pane_pid: 99,
                codex_session_id: None,
                codex_thread_id: None,
            },
            cwd: "/tmp/project".into(),
        }
    }

    /// A dsh anchor with no observed App Server endpoint is complete on its own.
    #[test]
    fn dsh_anchor_alone_requires_no_appserver_fields() {
        let facts = IdentityFacts {
            dsh_session_id: Some("session-1".into()),
            ..IdentityFacts::default()
        };
        assert!(required_fields(&facts).is_empty());
    }

    /// A tmux anchor with no observed endpoint is complete on its own.
    #[test]
    fn tmux_anchor_alone_requires_no_appserver_fields() {
        let facts = IdentityFacts {
            tmux: Some(tmux_anchor()),
            ..IdentityFacts::default()
        };
        assert!(required_fields(&facts).is_empty());
    }

    /// An observed App Server endpoint selects the App Server candidate, which
    /// reads all four fields unconditionally. The anchor must not short-circuit
    /// that request: doing so reaches `expect("complete native facts")` and
    /// panics the daemon handler instead of asking the caller.
    #[test]
    fn an_observed_endpoint_still_requires_the_four_appserver_fields() {
        let facts = IdentityFacts {
            dsh_session_id: Some("session-1".into()),
            endpoint: Some("unix:///tmp/appserver.sock".into()),
            ..IdentityFacts::default()
        };
        assert_eq!(
            required_fields(&facts),
            vec!["session_id", "thread_id", "namespace"]
        );
        let facts = IdentityFacts {
            tmux: Some(tmux_anchor()),
            endpoint: Some("unix:///tmp/appserver.sock".into()),
            ..IdentityFacts::default()
        };
        assert_eq!(
            required_fields(&facts),
            vec!["session_id", "thread_id", "namespace"]
        );
    }

    /// With no anchor and no endpoint all four facts are requested.
    #[test]
    fn no_anchor_requests_all_four_fields() {
        let facts = IdentityFacts::default();
        assert_eq!(
            required_fields(&facts),
            vec!["session_id", "thread_id", "endpoint", "namespace"]
        );
    }

    #[test]
    fn typed_operation_request_preserves_action_intent_and_capability_shape() {
        let request = IdentityContextRequest {
            operation_id: "ctxop-typed".into(),
            invocation: "automatic".into(),
            action: "context".into(),
            facts: IdentityFacts::default(),
            approval: None,
            grant_approval: None,
            query: false,
            query_capability: "base64url:capability".into(),
            invocation_ticket: String::new(),
        };
        assert!(validate_operation_request(&request).is_ok());
        assert_eq!(
            serde_json::to_value(&request).unwrap()["operation_id"],
            "ctxop-typed"
        );
    }

    #[test]
    fn query_and_missing_capability_are_rejected_before_operation_admission() {
        let query = IdentityContextRequest {
            operation_id: "ctxop-query".into(),
            invocation: "query".into(),
            action: "query".into(),
            facts: IdentityFacts::default(),
            approval: None,
            grant_approval: None,
            query: true,
            query_capability: "base64url:capability".into(),
            invocation_ticket: String::new(),
        };
        assert_eq!(
            validate_operation_request(&query).unwrap_err(),
            "IDENTITY_OPERATION_QUERY_SHAPE_INVALID"
        );

        let missing_capability = IdentityContextRequest {
            invocation: "automatic".into(),
            action: "context".into(),
            query: false,
            query_capability: String::new(),
            invocation_ticket: String::new(),
            ..query
        };
        assert_eq!(
            validate_operation_request(&missing_capability).unwrap_err(),
            "IDENTITY_OPERATION_CAPABILITY_REQUIRED: mutating context requires a query capability"
        );
    }

    #[test]
    fn s23_dispatch_preserves_typed_identity_operation() {
        let (server, root) = test_server();
        let host_paths = server.host_paths.clone();
        let journal = Arc::new(OperationJournal::open(host_paths.journal_path()).unwrap());
        let manager = ProjectRuntimeManager::new_with_operation_journal(
            Arc::new(server),
            &host_paths,
            journal.clone(),
        )
        .unwrap();
        let context = ProjectContext::for_registered_root_with_app(
            &root,
            AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
        )
        .unwrap();
        let request = IdentityContextRequest {
            operation_id: "ctxop-dispatch".into(),
            invocation: "automatic".into(),
            action: "context".into(),
            facts: IdentityFacts {
                tmux: Some(TmuxCandidate {
                    endpoint: crate::proto::TmuxEndpoint {
                        socket_path: "/tmp/tmux-ctxop.sock".into(),
                        server_pid: 42,
                        tmux_session_id: "$7".into(),
                        pane_id: "%3".into(),
                        pane_pid: 99,
                        codex_session_id: None,
                        codex_thread_id: None,
                    },
                    cwd: root.display().to_string(),
                }),
                ..IdentityFacts::default()
            },
            approval: None,
            grant_approval: None,
            query: false,
            query_capability: "base64url:capability".into(),
            invocation_ticket: String::new(),
        };
        inject_next_append_fault();
        let (_, response) = manager.identity_context(context.clone(), request.clone());
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("IDENTITY_OPERATION_DURABILITY_FAILED: injected operation journal append failure")
        );
        let replay = OperationJournal::open(host_paths.journal_path()).unwrap();
        let query = replay
            .query(
                &IdentityContextRequest {
                    operation_id: request.operation_id.clone(),
                    invocation: "query".into(),
                    action: "query".into(),
                    facts: IdentityFacts::default(),
                    approval: None,
                    grant_approval: None,
                    query: true,
                    query_capability: request.query_capability.clone(),
                    invocation_ticket: String::new(),
                },
                context.project_scope.as_str(),
                context.app_scope_id.as_str(),
            )
            .unwrap_err();
        assert!(query.starts_with("IDENTITY_OPERATION_UNKNOWN"), "{query}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn same_key_retry_returns_the_retained_projection_without_reconciliation() {
        let (server, root) = test_server();
        let host_paths = server.host_paths.clone();
        let journal = Arc::new(OperationJournal::open(host_paths.journal_path()).unwrap());
        let manager = ProjectRuntimeManager::new_with_operation_journal(
            Arc::new(server),
            &host_paths,
            journal.clone(),
        )
        .unwrap();
        let context = ProjectContext::for_registered_root_with_app(
            &root,
            AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
        )
        .unwrap();
        let request = IdentityContextRequest {
            operation_id: "ctxop-noop".into(),
            invocation: "automatic".into(),
            action: "context".into(),
            facts: IdentityFacts::default(),
            approval: None,
            grant_approval: None,
            query: false,
            query_capability: "base64url:capability".into(),
            invocation_ticket: String::new(),
        };
        let admitted = journal
            .append(crate::server::operation_journal::OperationAdmission {
                operation_id: request.operation_id.clone(),
                project_scope: context.project_scope.as_str().to_owned(),
                app_scope_id: context.app_scope_id.as_str().to_owned(),
                action: request.action.clone(),
                invocation: request.invocation.clone(),
                intent_digest: identity_operation_intent_digest(&context, &request),
                query_capability_hash: crate::server::operation_journal::capability_hash(
                    &request.query_capability,
                ),
                phase: crate::proto::IdentityOperationPhase::Admitted,
                committed_phases: Vec::new(),
                nested_command_id: None,
                nested_operation_id: None,
                approval_evidence: None,
            })
            .unwrap();
        let before = std::fs::read(host_paths.journal_path()).unwrap();
        let (_, response) = manager.identity_context(context, request);
        let after = std::fs::read(host_paths.journal_path()).unwrap();
        // The retained projection is honestly incomplete: outer `ok` is false
        // for the `unknown` outcome, and the same key neither appends nor
        // dispatches a side effect.
        assert!(!response.ok, "{:?}", response.error);
        assert_eq!(before, after, "same-key retry must not append or dispatch");
        assert_eq!(
            response.data["result"]["operation_id"],
            admitted.operation_id
        );
        assert_eq!(response.data["result"]["outcome"], "unknown");
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn b03_transport(thread: &str) -> crate::proto::SelectedTransport {
        crate::server::peer_tests::test_appserver_transport(thread)
    }

    fn b03_journal(server: &crate::server::Server) -> Arc<OperationJournal> {
        Arc::new(OperationJournal::open(server.host_paths.journal_path()).unwrap())
    }

    /// The exact prepared envelope is what gets consumed: the receipt's nested
    /// command/operation IDs equal the IDs carried on the prepared envelope.
    #[test]
    fn s23_b03_prepared_register_envelope_ids_equal_the_consumed_receipt() {
        let (server, root) = test_server();
        let worker_id = "b03-worker-1";
        let token = "b03-token";
        let transport = b03_transport("thread-b03-1");
        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        let app_scope = AppServerId::new("tui-default").unwrap();
        let prepared = server
            .prepare_register_envelope_for_scope(
                worker_id,
                token,
                &transport,
                project_scope.clone(),
                &root.to_string_lossy(),
                app_scope.clone(),
                false,
            )
            .unwrap();
        let prepared_command = prepared.nested_command_id.clone();
        let prepared_operation = prepared.nested_operation_id.clone();
        let response =
            consume_prepared_register_typed(&server, worker_id, &transport, prepared, None);
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.data["command_id"], prepared_command);
        assert_eq!(response.data["operation_id"], prepared_operation);
        assert_eq!(response.data["replayed"], false);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// If the durable outer `Validating` sync fails, Register is never
    /// consumed: no worker is registered and no nested receipt exists.
    #[test]
    fn s23_b03_outer_sync_failure_prevents_register_consume() {
        let (server, root) = test_server();
        let worker_id = "b03-worker-2";
        let token = "b03-token";
        let transport = b03_transport("thread-b03-2");
        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        let app_scope = AppServerId::new("tui-default").unwrap();
        let journal = b03_journal(&server);
        journal
            .append(crate::server::operation_journal::OperationAdmission {
                operation_id: "ctxop-b03-bind".into(),
                project_scope: project_scope.as_str().to_owned(),
                app_scope_id: app_scope.as_str().to_owned(),
                action: "context".into(),
                invocation: "automatic".into(),
                intent_digest: "digest".into(),
                query_capability_hash: crate::server::operation_journal::capability_hash("cap"),
                phase: crate::proto::IdentityOperationPhase::Admitted,
                committed_phases: Vec::new(),
                nested_command_id: None,
                nested_operation_id: None,
                approval_evidence: None,
            })
            .unwrap();
        let binding = RegisterOuterBinding::new(journal, "ctxop-b03-bind".into());
        inject_next_append_fault();
        let mut before_consume =
            |prepared: &PreparedRegisterEnvelope| binding.bind_validating(prepared);
        let response = register_typed_observed(
            &server,
            worker_id,
            token,
            &transport,
            &root.to_string_lossy(),
            Some(project_scope),
            Some(app_scope),
            false,
            None,
            Some(&mut before_consume),
        );
        assert!(!response.ok, "{response:?}");
        assert!(
            response
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("IDENTITY_OPERATION_DURABILITY_FAILED")),
            "{:?}",
            response.error
        );
        assert!(binding.bound().is_none());
        assert!(!server.state.lock().unwrap().workers.contains_key(worker_id));
        assert!(server
            .state
            .lock()
            .unwrap()
            .global
            .command_receipts
            .is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The pure readback returns the real inner receipt only when the nested
    /// command id exists and the nested operation id matches exactly.
    #[test]
    fn s23_b03_inner_receipt_readback_requires_exact_nested_ids() {
        let (server, root) = test_server();
        let host_paths = server.host_paths.clone();
        let journal = Arc::new(OperationJournal::open(host_paths.journal_path()).unwrap());
        let manager = ProjectRuntimeManager::new_with_operation_journal(
            Arc::new(server),
            &host_paths,
            journal.clone(),
        )
        .unwrap();
        let worker_id = "b03-worker-3";
        let token = "b03-token";
        let transport = b03_transport("thread-b03-3");
        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        let app_scope = AppServerId::new("tui-default").unwrap();
        let context =
            ProjectContext::for_registered_root_with_app(&root, app_scope.clone()).unwrap();
        manager.install_runtime(
            &(
                app_scope.as_str().to_owned(),
                project_scope.as_str().to_owned(),
            ),
            manager.host.clone(),
            None,
        );
        let prepared = manager
            .host
            .prepare_register_envelope_for_scope(
                worker_id,
                token,
                &transport,
                project_scope.clone(),
                &root.to_string_lossy(),
                app_scope,
                false,
            )
            .unwrap();
        let command_id = prepared.nested_command_id.clone();
        let operation_id = prepared.nested_operation_id.clone();
        let response =
            consume_prepared_register_typed(&manager.host, worker_id, &transport, prepared, None);
        assert!(
            response.ok,
            "error={:?} data={}",
            response.error, response.data
        );

        let projection = crate::proto::IdentityOperationProjection {
            operation_id: "ctxop-b03-read".into(),
            phase: crate::proto::IdentityOperationPhase::InnerDispatched,
            outcome: "unknown".into(),
            committed_phases: vec![crate::proto::IdentityOperationPhase::Validating],
            business_receipts: Vec::new(),
            nested_command_id: Some(command_id.clone()),
            nested_operation_id: Some(operation_id.clone()),
        };
        let readback = manager
            .inner_register_receipt_readback(&context, &projection)
            .unwrap();
        assert_eq!(readback["command_id"], command_id);
        assert_eq!(readback["operation_id"], operation_id);

        // A mismatched nested operation id must not be shown.
        let wrong = crate::proto::IdentityOperationProjection {
            nested_operation_id: Some("register-op-wrong".into()),
            ..projection.clone()
        };
        assert!(manager
            .inner_register_receipt_readback(&context, &wrong)
            .is_none());

        // A missing command id is never guessed.
        let missing = crate::proto::IdentityOperationProjection {
            nested_command_id: None,
            ..projection
        };
        assert!(manager
            .inner_register_receipt_readback(&context, &missing)
            .is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A revision change between prepare and consume is rejected by the
    /// existing CAS: no worker is registered, no receipt is created, and the
    /// prepared nested IDs are not rewritten or retried.
    #[test]
    fn s23_b03_stale_prepared_revision_is_rejected_without_effect_or_reprepare() {
        let (server, root) = test_server();
        let worker_id = "b03-worker-4";
        let token = "b03-token";
        let transport = b03_transport("thread-b03-4");
        let project_scope = GlobalState::canonical_project_scope(&root).unwrap();
        let app_scope = AppServerId::new("tui-default").unwrap();
        let journal = b03_journal(&server);
        journal
            .append(crate::server::operation_journal::OperationAdmission {
                operation_id: "ctxop-b03-stale".into(),
                project_scope: project_scope.as_str().to_owned(),
                app_scope_id: app_scope.as_str().to_owned(),
                action: "context".into(),
                invocation: "automatic".into(),
                intent_digest: "digest".into(),
                query_capability_hash: crate::server::operation_journal::capability_hash("cap"),
                phase: crate::proto::IdentityOperationPhase::Admitted,
                committed_phases: Vec::new(),
                nested_command_id: None,
                nested_operation_id: None,
                approval_evidence: None,
            })
            .unwrap();
        let binding = RegisterOuterBinding::new(journal, "ctxop-b03-stale".into());
        let prepared = server
            .prepare_register_envelope_for_scope(
                worker_id,
                token,
                &transport,
                project_scope.clone(),
                &root.to_string_lossy(),
                app_scope.clone(),
                false,
            )
            .unwrap();
        let prepared_command = prepared.nested_command_id.clone();
        let prepared_operation = prepared.nested_operation_id.clone();
        binding.bind_validating(&prepared).unwrap();
        assert_eq!(
            binding
                .bound()
                .as_ref()
                .map(|(command, _)| command.as_str()),
            Some(prepared_command.as_str())
        );
        // A concurrent project mutation advances the reducer revision after
        // prepare, so the prepared envelope's CAS must now fail.
        server
            .commit_checked(&[Event::KeepaliveUpdated {
                worker_id: "b03-other-worker".into(),
                record: crate::server::keepalive::Record::default(),
            }])
            .unwrap();
        let response = server.consume_prepared_register(prepared).unwrap_err();
        assert!(response.to_string().contains("revision"), "{response}");
        let state = server.state.lock().unwrap();
        assert!(!state.workers.contains_key(worker_id));
        assert!(!state
            .global
            .lookup_command_receipt(&CommandId::new(prepared_command.clone()).unwrap())
            .is_some());
        drop(state);
        // The bound IDs stay exactly as prepared; no reprepare/new suffix.
        assert_eq!(
            binding.bound(),
            Some((prepared_command, prepared_operation))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn approved_identity_digest_excludes_descriptive_metadata_but_binds_facts_and_fence() {
        let facts = IdentityFacts {
            thread_id: Some("thread-a".into()),
            ..Default::default()
        };
        let mut approval = json!({
            "decision":"approved", "decided_by":"user", "target_identity":"peer-a",
            "project_scope":"/project", "app_scope_id":"appserver-cli",
            "action":"restore_identity",
            "expected_incumbent":{"binding_id":"bind-7", "endpoint_generation":7},
            "intent_digest":"sha256:placeholder", "approved_at_ms":1770000000000_i64
        });
        let expected = approval_digest(&approval, &facts);
        approval["intent_digest"] = expected.clone().into();
        assert_eq!(
            parse_identity_approval(&approval).unwrap().intent_digest,
            expected
        );
        approval["decided_by"] = "operator".into();
        approval["approved_at_ms"] = 1770000000001_i64.into();
        assert_eq!(approval_digest(&approval, &facts), expected);
        approval["expected_incumbent"]["endpoint_generation"] = 8.into();
        assert_ne!(approval_digest(&approval, &facts), expected);
        assert_ne!(
            approval_digest(&approval, &IdentityFacts::default()),
            expected
        );
    }

    #[test]
    fn approved_recovery_without_durable_incumbent_is_denied_before_admission() {
        let (server, root) = test_server();
        let host_paths = server.host_paths.clone();
        let journal = Arc::new(OperationJournal::open(host_paths.journal_path()).unwrap());
        let manager = ProjectRuntimeManager::new_with_operation_journal(
            Arc::new(server),
            &host_paths,
            journal.clone(),
        )
        .unwrap();
        let context = ProjectContext::for_registered_root_with_app(
            &root,
            AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
        )
        .unwrap();
        let facts = IdentityFacts {
            tmux: Some(tmux_anchor()),
            ..Default::default()
        };
        let mut approval = json!({
            "decision":"approved", "decided_by":"user", "target_identity":"peer-missing",
            "project_scope":context.project_scope.as_str(), "app_scope_id":context.app_scope_id.as_str(),
            "action":"restore_identity",
            "expected_incumbent":{"binding_id":"bind-7", "endpoint_generation":7},
            "intent_digest":"sha256:placeholder", "approved_at_ms":1770000000000_i64
        });
        approval["intent_digest"] = approval_digest(&approval, &facts).into();
        let request = IdentityContextRequest {
            operation_id: "ctxop-approved-no-incumbent".into(),
            invocation: "approved_recovery".into(),
            // The request action is the context operation; the approval's own
            // `action` field carries `restore_identity` / `replace_binding`.
            action: "context".into(),
            facts,
            approval: Some(approval),
            grant_approval: None,
            query: false,
            query_capability: "base64url:capability".into(),
            invocation_ticket: String::new(),
        };
        let (_, response) = manager.identity_context(context.clone(), request.clone());
        assert!(!response.ok);
        assert_eq!(
            response.error.as_deref(),
            Some("IDENTITY_APPROVAL_IDENTITY_NOT_FOUND")
        );
        assert!(journal
            .query(
                &request,
                context.project_scope.as_str(),
                context.app_scope_id.as_str()
            )
            .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// One verified AppServer candidate for the fake socket the test serves.
    fn appserver_candidate(
        root: &Path,
        endpoint: &str,
        session: &str,
        thread: &str,
    ) -> TransportCandidates {
        TransportCandidates {
            appserver: Some(AppServerCandidate {
                endpoint: format!("unix://{endpoint}"),
                namespace: "codex_tui".into(),
                session_id: session.into(),
                thread_id: thread.into(),
                cwd: root.to_string_lossy().into_owned(),
            }),
            tmux: None,
            dsh: None,
        }
    }

    fn recovery_facts(endpoint: &str, session: &str, thread: &str) -> IdentityFacts {
        IdentityFacts {
            session_id: Some(session.into()),
            thread_id: Some(thread.into()),
            endpoint: Some(format!("unix://{endpoint}")),
            namespace: Some("codex_tui".into()),
            ..IdentityFacts::default()
        }
    }

    /// Serve the exact AppServer admission handshake (`initialize` then
    /// `thread/read`) so the daemon's real `verify_candidate` accepts the
    /// candidate without any production fault seam.
    fn serve_fake_appserver(
        listener: std::os::unix::net::UnixListener,
        session: &str,
        thread: &str,
        cwd: &Path,
        connections: usize,
    ) -> std::thread::JoinHandle<()> {
        use std::io::{Read, Write};
        let session = session.to_owned();
        let thread = thread.to_owned();
        let cwd = cwd.to_string_lossy().into_owned();
        std::thread::spawn(move || {
            for _ in 0..connections {
                let (mut stream, _) = listener.accept().unwrap();
                let mut header = Vec::new();
                let mut byte = [0_u8; 1];
                while !header.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                stream
                    .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
                    .unwrap();
                loop {
                    let request = read_test_frame(&mut stream);
                    let request: serde_json::Value = serde_json::from_slice(&request).unwrap();
                    let Some(id) = request.get("id").cloned() else {
                        continue;
                    };
                    let method = request["method"].as_str().unwrap();
                    let response = match method {
                        "initialize" => json!({"id": id, "result": {}}),
                        "thread/read" => {
                            json!({
                                "id": id,
                                "result": {"thread": {"id": thread, "sessionId": session, "cwd": cwd}}
                            })
                        }
                        "thread/items/list" => {
                            json!({"id": id, "error": {"code": -32601, "message": "unsupported"}})
                        }
                        "turn/start" | "turn/steer" => {
                            json!({"id": id, "error": {"code": -32600, "message": "invalid params"}})
                        }
                        "thread/turns/list" => json!({"id": id, "result": {"data": []}}),
                        other => panic!("unexpected AppServer method {other}"),
                    };
                    stream
                        .write_all(&encode_test_frame(
                            0x1,
                            &serde_json::to_vec(&response).unwrap(),
                        ))
                        .unwrap();
                    if method == "thread/turns/list" {
                        break;
                    }
                }
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        })
    }

    fn read_test_frame(stream: &mut std::os::unix::net::UnixStream) -> Vec<u8> {
        use std::io::Read;
        let mut header = [0_u8; 2];
        stream.read_exact(&mut header).unwrap();
        let masked = header[1] & 0x80 != 0;
        let mut length = (header[1] & 0x7f) as usize;
        if length == 126 {
            let mut bytes = [0_u8; 2];
            stream.read_exact(&mut bytes).unwrap();
            length = u16::from_be_bytes(bytes) as usize;
        }
        let mut mask = [0_u8; 4];
        if masked {
            stream.read_exact(&mut mask).unwrap();
        }
        let mut payload = vec![0_u8; length];
        stream.read_exact(&mut payload).unwrap();
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        payload
    }

    fn encode_test_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mut frame = vec![0x80 | opcode];
        let length = payload.len();
        if length < 126 {
            frame.push(0x80 | length as u8);
        } else if length <= u16::MAX as usize {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
        let mask = [0x11_u8, 0x22, 0x33, 0x44];
        frame.extend_from_slice(&mask);
        for (index, byte) in payload.iter().enumerate() {
            frame.push(byte ^ mask[index % 4]);
        }
        frame
    }

    fn test_socket_path(worker_id: &str, generation: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "collab-d2i-{}-{worker_id}-{generation}.sock",
            std::process::id()
        ))
    }

    /// True when this sandbox permits binding a unix socket. The managed
    /// sandbox denies it, so real-transport owner tests skip rather than
    /// report a false failure; the same pattern is used by the adapter tests.
    fn unix_sockets_available() -> bool {
        let probe = std::env::temp_dir().join(format!(
            "collab-d2i-probe-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        match std::os::unix::net::UnixListener::bind(&probe) {
            Ok(listener) => {
                drop(listener);
                let _ = std::fs::remove_file(&probe);
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
            Err(error) => panic!("probe unix socket bind: {error}"),
        }
    }

    /// Owner-boundary proof consumption and the exact incumbent fence recheck,
    /// exercised without a live transport. The Register owner must accept the
    /// proof only for its exact target and route scope, and the fence must name
    /// the committed incumbent binding and generation.
    #[test]
    fn register_approval_proof_is_scoped_and_rechecks_the_exact_incumbent_fence() {
        let project_scope = ProjectScopeId::new("/tmp/collab-d2i-fence".to_owned()).unwrap();
        let app_scope = AppServerId::new("appserver-cli").unwrap();
        let other_app_scope = AppServerId::new("tui-default").unwrap();
        let binding_id = BindingId::new("binding-peer-a").unwrap();
        let mut state = State::default();
        state
            .global
            .register_project(
                ProjectRegistration::with_registered_at(
                    project_scope.clone(),
                    app_scope.clone(),
                    1,
                )
                .unwrap(),
            )
            .unwrap();
        state
            .global
            .bind_runtime(
                RuntimeBinding::new_with_session(
                    project_scope.clone(),
                    app_scope.clone(),
                    AgentId::new("peer-a").unwrap(),
                    RuntimeId::new("runtime-peer-a").unwrap(),
                    binding_id.clone(),
                    7,
                    Some(crate::identity::SessionId::new("session-peer-a").unwrap()),
                    Some(NativeThreadId::new("thread-peer-a").unwrap()),
                )
                .unwrap(),
            )
            .unwrap();

        let proof = RegisterApprovalProof {
            target_identity: "peer-a".into(),
            project_scope: project_scope.as_str().to_owned(),
            app_scope_id: app_scope.as_str().to_owned(),
            incumbent_binding_id: binding_id.as_str().to_owned(),
            incumbent_endpoint_generation: 7,
        };
        assert!(approved_register_authorized(
            Some(&proof),
            "peer-a",
            Some(&project_scope),
            Some(&app_scope),
        ));
        assert!(!approved_register_authorized(
            Some(&proof),
            "peer-b",
            Some(&project_scope),
            Some(&app_scope),
        ));
        assert!(!approved_register_authorized(
            Some(&proof),
            "peer-a",
            Some(&project_scope),
            Some(&other_app_scope),
        ));
        assert!(!approved_register_authorized(
            None,
            "peer-a",
            Some(&project_scope),
            Some(&app_scope),
        ));
        assert!(approved_register_fence_current(
            &state,
            &proof,
            &project_scope,
            &app_scope,
        ));
        let stale = RegisterApprovalProof {
            incumbent_endpoint_generation: 8,
            ..proof.clone()
        };
        assert!(!approved_register_fence_current(
            &state,
            &stale,
            &project_scope,
            &app_scope,
        ));
        let unknown_binding = RegisterApprovalProof {
            incumbent_binding_id: "binding-missing".into(),
            ..proof
        };
        assert!(!approved_register_fence_current(
            &state,
            &unknown_binding,
            &project_scope,
            &app_scope,
        ));
    }

    /// Spawn the fake AppServer that answers the recovery Register's own
    /// `verify_candidate`, returning the request candidates bound to it.
    fn recovery_appserver(
        endpoint: &Path,
        session: &str,
        thread: &str,
        cwd: &Path,
        connections: usize,
    ) -> std::thread::JoinHandle<()> {
        let listener = std::os::unix::net::UnixListener::bind(endpoint).unwrap();
        serve_fake_appserver(listener, session, thread, cwd, connections)
    }

    /// Register and promote the daemon's current credential, then persist a
    /// stale local credential and return a separate recovery endpoint.
    fn stale_master_incumbent(
        server: &Server,
        root: &Path,
        worker_id: &str,
    ) -> (IdentityFacts, BindingId, u64, String, PathBuf, String) {
        let first_socket = test_socket_path(worker_id, "1");
        let first_listener = std::os::unix::net::UnixListener::bind(&first_socket).unwrap();
        let first_thread = format!("thread-{worker_id}-1");
        let first_server = serve_fake_appserver(
            first_listener,
            &format!("session-{worker_id}-1"),
            &first_thread,
            root,
            1,
        );
        let first_endpoint = first_socket.to_string_lossy().into_owned();
        let first = handle_register_with_app_scope(
            server,
            worker_id.into(),
            "token-1".into(),
            root.to_string_lossy().into_owned(),
            Some(AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap()),
            Some(appserver_candidate(
                root,
                &first_endpoint,
                &format!("session-{worker_id}-1"),
                &first_thread,
            )),
        );
        assert!(first.ok, "{:?}", first.error);
        first_server.join().unwrap();
        let promote = handle_master_promote(
            server,
            worker_id.into(),
            "token-1".into(),
            "user-approved master promotion".into(),
        );
        assert!(promote.ok, "{:?}", promote.error);
        let scope = ProjectContext::for_registered_root_with_app(
            root,
            AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
        )
        .unwrap();
        let binding = {
            let state = server.state.lock().unwrap();
            state
                .global
                .lookup_binding_for(
                    &RouteScope {
                        app_scope_id: scope.app_scope_id.clone(),
                        project_scope_id: scope.project_scope.clone(),
                    },
                    &BindingId::new(format!("binding-{worker_id}")).unwrap(),
                )
                .cloned()
                .expect("registered binding")
        };
        let transport = {
            let state = server.state.lock().unwrap();
            selected_transport_for_worker(state.workers.get(worker_id).expect("worker"))
                .expect("selected transport")
        };
        let recovery_socket = test_socket_path(worker_id, "recovery");
        let recovery_endpoint = recovery_socket.to_string_lossy().into_owned();
        let recovery_session = format!("session-{worker_id}-recovery");
        let recovery_thread = format!("thread-{worker_id}-recovery");
        let mut identity = identity::Identity {
            worker_id: worker_id.into(),
            token: "token-2".into(),
            project_scope: Some(scope.project_scope.clone()),
            runtime: None,
            transport: None,
        };
        identity::persist_registration_at(
            &server.host_paths,
            &Scope {
                root: root.to_path_buf(),
            },
            &mut identity,
            RuntimeIdentity {
                agent_id: binding.agent_id.clone(),
                runtime_id: binding.runtime_id.clone(),
                appserver_id: binding.app_scope_id.clone(),
                endpoint_generation: binding.endpoint_generation,
                binding_id: binding.binding_id.clone(),
                session_id: binding.session_id.clone(),
                native_thread_id: binding.native_thread_id.clone(),
            },
            transport.clone(),
        )
        .unwrap();
        (
            recovery_facts(&recovery_endpoint, &recovery_session, &recovery_thread),
            binding.binding_id.clone(),
            binding.endpoint_generation,
            binding.agent_id.as_str().to_owned(),
            recovery_socket,
            recovery_thread,
        )
    }

    fn recovery_manager(server: Arc<Server>) -> Arc<ProjectRuntimeManager> {
        let host_paths = server.host_paths.clone();
        let journal = Arc::new(OperationJournal::open(host_paths.journal_path()).unwrap());
        ProjectRuntimeManager::new_with_operation_journal(server, &host_paths, journal).unwrap()
    }

    fn recovery_context(root: &Path) -> ProjectContext {
        ProjectContext::for_registered_root_with_app(
            root,
            AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
        )
        .unwrap()
    }

    fn identity_approval(
        context: &ProjectContext,
        facts: &IdentityFacts,
        target: &str,
        incumbent_binding_id: &str,
        incumbent_generation: u64,
    ) -> serde_json::Value {
        let mut approval = json!({
            "decision":"approved",
            "decided_by":"user",
            "target_identity": target,
            "project_scope": context.project_scope.as_str(),
            "app_scope_id": context.app_scope_id.as_str(),
            "action":"replace_binding",
            "expected_incumbent":{
                "binding_id": incumbent_binding_id,
                "endpoint_generation": incumbent_generation
            },
            "intent_digest":"sha256:placeholder",
            "approved_at_ms":1770000000000_i64
        });
        approval["intent_digest"] = approval_digest(&approval, facts).into();
        approval
    }

    fn grant_approval(
        context: &ProjectContext,
        facts: &IdentityFacts,
        target: &str,
        grant_id: &str,
        grant_generation: u64,
    ) -> serde_json::Value {
        let mut approval = json!({
            "decision":"approved",
            "decided_by":"user",
            "target_identity": target,
            "project_scope": context.project_scope.as_str(),
            "app_scope_id": context.app_scope_id.as_str(),
            "action":"replace_master_grant",
            "expected_grant":{"grant_id": grant_id, "generation": grant_generation},
            "intent_digest":"sha256:placeholder",
            "approved_at_ms":1770000000000_i64
        });
        approval["intent_digest"] = approval_digest(&approval, facts).into();
        approval
    }

    fn grant_for(
        server: &Server,
        root: &Path,
        binding_id: &BindingId,
    ) -> Option<crate::server::global_state::MasterGrant> {
        let context = recovery_context(root);
        let state = server.state.lock().unwrap();
        current_master_grant(
            &state,
            Some(&RouteScope {
                app_scope_id: context.app_scope_id,
                project_scope_id: context.project_scope,
            }),
        )
        .filter(|grant| grant.binding_id == *binding_id)
    }

    fn binding_generation_for(
        server: &Server,
        context: &ProjectContext,
        binding_id: &BindingId,
    ) -> u64 {
        server
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &RouteScope {
                    app_scope_id: context.app_scope_id.clone(),
                    project_scope_id: context.project_scope.clone(),
                },
                binding_id,
            )
            .expect("recovered binding")
            .endpoint_generation
    }

    /// Approved peer recovery consumes the daemon Register proof at the owner
    /// boundary, commits the exact binding, and keeps the existing master grant
    /// for an identity-only approval (no implicit promotion, clear, or replace).
    #[test]
    fn approved_register_recovery_commits_binding_and_retains_master_grant() {
        if !unix_sockets_available() {
            eprintln!("SKIP approved-register owner test: sandbox denied unix socket bind");
            return;
        }
        let (server, root) = test_server();
        let server = Arc::new(server);
        let manager = recovery_manager(server.clone());
        let (facts, binding_id, incumbent_generation, _agent, endpoint, thread) =
            stale_master_incumbent(&server, &root, "peer-a");
        let context = recovery_context(&root);
        let before_grant = grant_for(&server, &root, &binding_id).expect("master grant");
        let recovery_server = recovery_appserver(
            &endpoint,
            facts.session_id.as_deref().expect("recovery session"),
            &thread,
            &root,
            1,
        );
        let request = IdentityContextRequest {
            operation_id: "ctxop-approved-peer".into(),
            invocation: "approved_recovery".into(),
            action: "context".into(),
            facts: facts.clone(),
            approval: Some(identity_approval(
                &context,
                &facts,
                "peer-a",
                binding_id.as_str(),
                incumbent_generation,
            )),
            grant_approval: None,
            query: false,
            query_capability: "base64url:capability".into(),
            invocation_ticket: String::new(),
        };
        let (runtime, response) = manager.identity_context(context.clone(), request);
        if response.ok {
            recovery_server.join().unwrap();
        } else {
            drop(recovery_server);
        }
        let host_token = server
            .state
            .lock()
            .unwrap()
            .workers
            .get("peer-a")
            .map(|worker| worker.token.clone());
        let runtime_token = runtime
            .state
            .lock()
            .unwrap()
            .workers
            .get("peer-a")
            .map(|worker| worker.token.clone());
        assert!(
            response.ok,
            "error={:?} data={} host_token={host_token:?} runtime_token={runtime_token:?}",
            response.error, response.data
        );
        assert_eq!(response.data["result"]["outcome"], "completed");
        assert_eq!(response.data["result"]["phase"], "context_complete");
        let new_generation = binding_generation_for(&server, &context, &binding_id);
        assert!(
            new_generation > incumbent_generation,
            "recovery must advance the endpoint generation"
        );
        {
            let state = server.state.lock().unwrap();
            let binding = state
                .global
                .lookup_binding_for(
                    &RouteScope {
                        app_scope_id: context.app_scope_id.clone(),
                        project_scope_id: context.project_scope.clone(),
                    },
                    &binding_id,
                )
                .expect("recovered binding");
            assert_eq!(binding.endpoint_generation, new_generation);
            assert_eq!(
                state
                    .workers
                    .get("peer-a")
                    .map(|worker| worker.token.as_str()),
                Some("token-2")
            );
        }
        let after_grant = grant_for(&server, &root, &binding_id).expect("retained grant");
        assert_eq!(after_grant.agent_id, before_grant.agent_id);
        assert_eq!(after_grant.binding_id, before_grant.binding_id);
        assert_eq!(after_grant.endpoint_generation, new_generation);
        assert_eq!(after_grant.granted_by, before_grant.granted_by);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A changed incumbent fence is refused with the accepted typed conflict
    /// before any owner effect, so no binding, credential, or grant moves.
    #[test]
    fn stale_incumbent_fence_is_refused_with_no_side_effects() {
        if !unix_sockets_available() {
            eprintln!("SKIP stale-incumbent owner test: sandbox denied unix socket bind");
            return;
        }
        let (server, root) = test_server();
        let server = Arc::new(server);
        let manager = recovery_manager(server.clone());
        let (facts, binding_id, incumbent_generation, _agent, _endpoint, _thread) =
            stale_master_incumbent(&server, &root, "peer-stale");
        let context = recovery_context(&root);
        let before_grant = grant_for(&server, &root, &binding_id).expect("master grant");
        let request = IdentityContextRequest {
            operation_id: "ctxop-approved-stale".into(),
            invocation: "approved_recovery".into(),
            action: "context".into(),
            facts: facts.clone(),
            approval: Some(identity_approval(
                &context,
                &facts,
                "peer-stale",
                binding_id.as_str(),
                incumbent_generation + 1,
            )),
            grant_approval: None,
            query: false,
            query_capability: "base64url:capability".into(),
            invocation_ticket: String::new(),
        };
        let (_, response) = manager.identity_context(context.clone(), request.clone());
        assert!(!response.ok);
        assert_eq!(response.data["result"]["outcome"], "denied");
        assert_eq!(
            response.data["result"]["owner_readback"]["error"],
            "APPROVAL_STALE_CONFLICT"
        );
        {
            let state = server.state.lock().unwrap();
            let binding = state
                .global
                .lookup_binding_for(
                    &RouteScope {
                        app_scope_id: context.app_scope_id.clone(),
                        project_scope_id: context.project_scope.clone(),
                    },
                    &binding_id,
                )
                .expect("binding");
            assert_eq!(binding.endpoint_generation, incumbent_generation);
        }
        let after_grant = grant_for(&server, &root, &binding_id).expect("grant");
        assert_eq!(after_grant, before_grant);
        assert!(manager
            .operation_journal
            .query(
                &request,
                context.project_scope.as_str(),
                context.app_scope_id.as_str()
            )
            .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// One invocation carrying both approvals validates them before admission,
    /// then commits the identity binding and the distinct grant replacement
    /// under their separate owners and reads both back before `completed`.
    #[test]
    fn dual_approval_commits_identity_and_separate_grant_replacement() {
        if !unix_sockets_available() {
            eprintln!("SKIP dual-approval owner test: sandbox denied unix socket bind");
            return;
        }
        let (server, root) = test_server();
        let server = Arc::new(server);
        let manager = recovery_manager(server.clone());
        let (facts, binding_id, incumbent_generation, _agent, endpoint, thread) =
            stale_master_incumbent(&server, &root, "master-a");
        let context = recovery_context(&root);
        let before_grant = grant_for(&server, &root, &binding_id).expect("master grant");
        let recovery_server = recovery_appserver(
            &endpoint,
            facts.session_id.as_deref().expect("recovery session"),
            &thread,
            &root,
            1,
        );
        let request = IdentityContextRequest {
            operation_id: "ctxop-approved-dual".into(),
            invocation: "approved_recovery".into(),
            action: "context".into(),
            facts: facts.clone(),
            approval: Some(identity_approval(
                &context,
                &facts,
                "master-a",
                binding_id.as_str(),
                incumbent_generation,
            )),
            grant_approval: Some(grant_approval(
                &context,
                &facts,
                "master-a",
                binding_id.as_str(),
                before_grant.endpoint_generation,
            )),
            query: false,
            query_capability: "base64url:capability".into(),
            invocation_ticket: String::new(),
        };
        let (_, response) = manager.identity_context(context.clone(), request);
        if response.ok {
            recovery_server.join().unwrap();
        } else {
            drop(recovery_server);
        }
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.data["result"]["outcome"], "completed");
        let new_generation = binding_generation_for(&server, &context, &binding_id);
        let after_grant = grant_for(&server, &root, &binding_id).expect("replacement grant");
        assert_eq!(after_grant.agent_id, before_grant.agent_id);
        assert_eq!(after_grant.binding_id, before_grant.binding_id);
        assert_eq!(after_grant.endpoint_generation, new_generation);
        assert_eq!(after_grant.granted_by, "user");
        assert_ne!(after_grant.approval, before_grant.approval);
        std::fs::remove_dir_all(root).unwrap();
    }
}
