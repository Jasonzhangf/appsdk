impl ProjectRuntimeManager {
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
