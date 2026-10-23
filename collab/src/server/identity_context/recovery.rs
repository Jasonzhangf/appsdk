impl ProjectRuntimeManager {
    fn recovered_binding_identity(
        &self,
        context: &ProjectContext,
        binding: &RuntimeBinding,
        state: &crate::server::state::State,
    ) -> Option<identity::Identity> {
        let worker = state.workers.get(binding.agent_id.as_str())?;
        Some(identity::Identity {
            worker_id: worker.id.clone(),
            token: worker.token.clone(),
            project_scope: Some(context.project_scope.clone()),
            runtime: Some(RuntimeIdentity {
                agent_id: binding.agent_id.clone(),
                runtime_id: binding.runtime_id.clone(),
                appserver_id: binding.app_scope_id.clone(),
                endpoint_generation: binding.endpoint_generation,
                binding_id: binding.binding_id.clone(),
                session_id: binding.session_id.clone(),
                native_thread_id: binding.native_thread_id.clone(),
            }),
            transport: worker.transport.clone(),
        })
    }

    /// Resolve the caller's current delivery placement from live, host-owned
    /// tmux endpoints. This is a read-only observation and runs only on the
    /// mutating admission/reconcile path.
    fn observe_identity_facts(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
    ) -> anyhow::Result<IdentityFacts> {
        let mut facts = facts.clone();
        let Some(caller_pid) = facts.caller_pid else {
            return Ok(facts);
        };
        let mut endpoints =
            crate::client::adapters::tmux::host_endpoints_at(self.host.host_paths.state_root())
                .map_err(anyhow::Error::msg)?;
        {
            let state = self.host.state.lock().unwrap();
            endpoints.extend(
                state
                    .global
                    .current_thread_routes
                    .values()
                    .chain(state.global.legacy_thread_routes.values())
                    .filter_map(|binding| {
                        binding
                            .tmux_endpoint
                            .as_ref()
                            .map(|endpoint| endpoint.socket_path.clone())
                    }),
            );
        }
        endpoints.sort();
        endpoints.dedup();
        let observed = crate::client::adapters::tmux::observe_caller(
            endpoints,
            caller_pid,
            facts.pane_hint.as_deref(),
        )
        .map_err(anyhow::Error::msg)?;
        // Current ancestry supersedes every caller-supplied or historical
        // placement address. No observation means no tmux selection.
        facts.tmux = observed.map(|mut endpoint| {
            // Native anchors remain transport metadata. Selecting the peer
            // still uses the observed project/session before these anchors.
            endpoint.codex_session_id = facts.session_id.clone();
            endpoint.codex_thread_id = facts.thread_id.clone();
            crate::proto::TmuxCandidate {
                endpoint,
                cwd: context.canonical_root.clone(),
            }
        });
        facts.tmux_anchor = None;
        // Observation is complete. Clearing the invocation-only inputs makes
        // later reconcile steps reuse the same live fact instead of observing
        // a second time. These inputs never enter an intent digest.
        facts.caller_pid = None;
        facts.pane_hint = None;
        Ok(facts)
    }

    fn current_session_identity(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
    ) -> anyhow::Result<Option<identity::Identity>> {
        let dsh_session = facts.dsh_session_id.as_deref()
            .or_else(|| facts.dsh.as_ref().map(|candidate| candidate.session_id.as_str()));
        let Some(session_id) = dsh_session.or(facts.session_id.as_deref()) else {
            return Ok(None);
        };
        if !self.routes.lock().unwrap().contains_key(&(
            context.app_scope_id.as_str().to_owned(),
            context.project_scope.as_str().to_owned(),
        )) {
            return Ok(None);
        }
        let runtime = self.select_runtime(context).map_err(anyhow::Error::msg)?;
        let state = runtime.state.lock().unwrap();
        let route = RouteScope {
            app_scope_id: context.app_scope_id.clone(),
            project_scope_id: context.project_scope.clone(),
        };
        let Some(project) = state.global.lookup_project_for_route(&route) else {
            return Ok(None);
        };
        let mut bindings = project
            .runtime_bindings
            .values()
            .filter(|binding| {
                binding
                    .session_id
                    .as_ref()
                    .is_some_and(|saved| saved.as_str() == session_id)
                    && state.workers.contains_key(binding.agent_id.as_str())
                    && (dsh_session.is_none() || state.workers.get(binding.agent_id.as_str())
                        .and_then(|worker| worker.transport.as_ref()).is_some_and(|transport| transport.kind == TransportKind::Dsh))
            })
            .collect::<Vec<_>>();
        if let Some(candidate) = facts.dsh.as_ref() {
            let had_session = !bindings.is_empty();
            bindings.retain(|binding| state.workers.get(binding.agent_id.as_str())
                .and_then(|worker| worker.transport.as_ref())
                .is_some_and(|transport| identity::dsh_transport_matches(candidate, transport)));
            if had_session && bindings.is_empty() {
                anyhow::bail!("IDENTITY_RESTORE_CONFLICT: DSH communication API differs from the current binding");
            }
        }
        let master = current_master_grant(&state, Some(&route));

        // Typed name occupancy is the authoritative current session owner.
        // Historical runtime bindings that merely carry the same session
        // address are not owner evidence and must not veto the selection.
        let typed_owner = typed_session_owner(&state, context.project_scope.as_str(), session_id)?;
        if let Some(worker_id) = typed_owner.as_deref() {
            let binding = bindings
                .iter()
                .copied()
                .find(|binding| binding.agent_id.as_str() == worker_id);
            if let Some(binding) = binding {
                return Ok(self.recovered_binding_identity(context, binding, &state));
            }
        }

        // A single current binding still resumes that worker. Multiple current
        // bindings with no unique typed owner are historical candidates only;
        // they must not be guessed and must not block a fresh stable carrier.
        if bindings.len() == 1 {
            let binding = bindings[0];
            return Ok(self.recovered_binding_identity(context, binding, &state));
        }
        if dsh_session.is_some() && bindings.len() > 1 {
            anyhow::bail!("IDENTITY_RESTORE_AMBIGUOUS: multiple current DSH bindings carry this session");
        }
        if let Some(binding) = bindings.iter().copied().find(|binding| {
            master
                .as_ref()
                .is_some_and(|grant| grant.binding_id == binding.binding_id)
        }) {
            return Ok(self.recovered_binding_identity(context, binding, &state));
        }
        Ok(None)
    }

    /// Migrate duplicate carriers even when the selected owner can restore
    /// directly without a new Register. Reuse Register's retirement events.
    fn converge_session_carriers(
        &self,
        runtime: &Arc<Server>,
        current: &RuntimeBinding,
    ) -> anyhow::Result<bool> {
        let events = {
            let mut state = runtime.state.lock().unwrap();
            let claimants = session_claimants(
                &state,
                current.agent_id.as_str(),
                &current.binding_id,
                &current.route_scope(),
                current,
            );
            let mut events = Vec::new();
            for claimant in claimants {
                events.extend(retire_claimant_events(
                    current.agent_id.as_str(),
                    &claimant,
                    "runtime session carrier reconciled by context",
                ).map_err(anyhow::Error::msg)?);
            }
            if !events.is_empty() {
                runtime.commit_locked(&mut state, &events)?;
            }
            events
        };
        let changed = !events.is_empty();
        if !Arc::ptr_eq(runtime, &self.host) {
            let publications = events.into_iter().filter(|event| matches!(
                event,
                Event::GlobalCurrentThreadRouteRetired { .. } | Event::GlobalRuntimeBound { .. }
            )).collect::<Vec<_>>();
            if !publications.is_empty() {
                self.host.commit_checked(&publications)?;
            }
        }
        Ok(changed)
    }

    /// Called only inside the existing name owner-start boundary. The
    /// receipt is the adjacent authenticated context result, not user payload.
    fn converge_context_session_carriers(
        &self,
        runtime: &Arc<Server>,
        context: &ProjectContext,
        response: &mut Resp,
    ) -> Result<(), String> {
        let identity: identity::Identity = serde_json::from_value(
            response.data["identity_receipt"].clone(),
        ).map_err(|error| format!("IDENTITY_RECEIPT_INVALID: {error}"))?;
        let binding = identity.runtime.as_ref().ok_or("IDENTITY_RUNTIME_MISSING")?;
        let saved = runtime.state.lock().unwrap().global.lookup_binding_for(
            &RouteScope {
                app_scope_id: context.app_scope_id.clone(),
                project_scope_id: context.project_scope.clone(),
            },
            &binding.binding_id,
        ).cloned().ok_or("IDENTITY_RUNTIME_MISSING")?;
        if self.converge_session_carriers(runtime, &saved).map_err(|error| error.to_string())? {
            let registered = ProjectContext::for_registered_route(
                Path::new(&context.canonical_root), binding,
            ).map_err(|error| error.to_string())?;
            let (_, mut snapshot) = self.dispatch_sync(Some(registered), Req::Context {
                worker_id: identity.worker_id,
                token: identity.token,
            });
            if !snapshot.ok {
                return Err(snapshot.error.unwrap_or_else(|| "CONTEXT_READBACK_FAILED".into()));
            }
            add_optional_master_choice(&mut snapshot.data);
            response.data["snapshot"] = snapshot.data;
        }
        Ok(())
    }

    fn refresh_session_endpoint(
        &self,
        runtime: &Server,
        saved: &RuntimeBinding,
        candidate: &crate::proto::TmuxCandidate,
        ident: &mut identity::Identity,
        scope: &Scope,
    ) -> anyhow::Result<RuntimeBinding> {
        let mut transport = runtime.state.lock().unwrap()
            .workers
            .get(&ident.worker_id)
            .and_then(|worker| worker.transport.clone())
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_TRANSPORT_MISSING"))?;
        transport.tmux_endpoint = Some(candidate.endpoint.clone());
        if transport.kind == TransportKind::Tmux {
            transport.endpoint = Some(candidate.endpoint.socket_path.clone());
            transport.namespace = Some(candidate.endpoint.tmux_session_id.clone());
            transport.session_id = candidate.endpoint.codex_session_id.clone();
            transport.thread_id = candidate.endpoint.codex_thread_id.clone();
        }

        self.refresh_session_transport(runtime, saved, transport, ident, scope)
    }

    fn refresh_session_transport(
        &self,
        runtime: &Server,
        saved: &RuntimeBinding,
        transport: SelectedTransport,
        ident: &mut identity::Identity,
        scope: &Scope,
    ) -> anyhow::Result<RuntimeBinding> {
        let mut state = runtime.state.lock().unwrap();
        let mut worker = state.workers.get(&ident.worker_id).cloned()
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_WORKER_MISSING"))?;
        let same_placement = saved.tmux_endpoint == transport.tmux_endpoint
            && saved.session_id.as_ref().map(identity::SessionId::as_str) == transport.session_id.as_deref()
            && saved.native_thread_id.as_ref().map(NativeThreadId::as_str) == transport.thread_id.as_deref()
            && (transport.kind == TransportKind::Tmux
                || worker.transport.as_ref().is_some_and(|old| old.kind == transport.kind
                    && old.endpoint == transport.endpoint && old.namespace == transport.namespace));
        if same_placement {
            let worker_changed = worker.transport.as_ref() != Some(&transport);
            if worker_changed {
                worker.transport = Some(transport.clone());
                runtime.commit_locked(&mut state, &[Event::Registered { worker }])?;
            }
            drop(state);
            if ident.transport.as_ref() != Some(&transport) {
                let binding = ident
                    .runtime
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("IDENTITY_RUNTIME_MISSING"))?;
                identity::persist_registration_at(
                    &self.host.host_paths,
                    scope,
                    ident,
                    binding,
                    transport,
                )?;
            }
            return Ok(saved.clone());
        }

        let mut next = saved.clone();
        next.endpoint_generation = next
            .endpoint_generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_ENDPOINT_GENERATION_EXHAUSTED"))?;
        next.tmux_endpoint = transport.tmux_endpoint.clone();
        next.session_id = transport.session_id.clone().map(identity::SessionId::new).transpose()?;
        next.native_thread_id = transport.thread_id.clone().map(NativeThreadId::new).transpose()?;
        worker.transport = Some(transport.clone());
        let grant = current_master_grant(&state, Some(&saved.route_scope()))
            .filter(|grant| grant.binding_id == saved.binding_id);
        let mut events = vec![
            Event::GlobalCurrentThreadRouteRetired {
                binding: saved.clone(),
            },
            Event::GlobalRuntimeBound {
                binding: next.clone(),
            },
            Event::Registered { worker },
        ];
        if let Some(mut grant) = grant {
            grant.endpoint_generation = next.endpoint_generation;
            events.push(Event::GlobalMasterGranted { grant });
        }
        runtime.commit_locked(&mut state, &events)?;
        drop(state);
        let mut publications = vec![Event::GlobalCurrentThreadRouteRetired {
            binding: saved.clone(),
        }];
        {
            let state = self.host.state.lock().unwrap();
            if let Some(previous) = transport.tmux_endpoint.as_ref()
                .and_then(|endpoint| state.global.lookup_unique_tmux_pane_route(endpoint))
            {
                if previous != saved {
                    // Retarget the delivery address only. The old project's
                    // worker, name, role grant and execution stay untouched.
                    publications.push(Event::GlobalCurrentThreadRouteRetired {
                        binding: previous.clone(),
                    });
                }
            }
        }
        publications.push(Event::GlobalCurrentThreadRouteSet {
            binding: next.clone(),
        });
        self.host.commit_checked(&publications)?;
        let mut binding = ident
            .runtime
            .clone()
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_RUNTIME_MISSING"))?;
        binding.endpoint_generation = next.endpoint_generation;
        binding.session_id = next.session_id.clone();
        binding.native_thread_id = next.native_thread_id.clone();
        identity::persist_registration_at(&self.host.host_paths, scope, ident, binding, transport)?;
        Ok(next)
    }
    fn current_anchor_binding(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
    ) -> anyhow::Result<Option<RuntimeBinding>> {
        let preferred = self
            .current_session_identity(context, facts)?
            .and_then(|identity| identity.runtime.map(|runtime| runtime.binding_id));
        let state = self.host.state.lock().unwrap();
        Ok(if let Some(session) = &facts.session_id {
            state
                .global
                .current_thread_routes
                .values()
                .find(|binding| {
                    binding.project_scope == context.project_scope
                        && binding.app_scope_id == context.app_scope_id
                        && preferred
                            .as_ref()
                            .is_none_or(|id| &binding.binding_id == id)
                        && binding
                            .session_id
                            .as_ref()
                            .is_some_and(|saved| saved.as_str() == session)
                        && match (&facts.thread_id, &binding.native_thread_id) {
                            (Some(thread), Some(saved)) => saved.as_str() == thread,
                            (Some(_), None) => false,
                            (None, _) => true,
                        }
                })
                .cloned()
        } else {
            None
        })
    }
    fn has_persisted_pane_route(&self, context: &ProjectContext, facts: &IdentityFacts) -> bool {
        facts.tmux.as_ref().is_some_and(|candidate| {
            self.resolve_route_by_tmux_endpoint(&candidate.endpoint)
                .is_ok_and(|route| {
                    route.project_scope == context.project_scope
                        && route.app_scope_id == context.app_scope_id
                })
        })
    }
    fn complete_persisted_tmux_facts(
        &self,
        context: &ProjectContext,
        facts: &mut IdentityFacts,
    ) -> anyhow::Result<()> {
        if facts.tmux.is_some() {
            return Ok(());
        }
        let Some(anchor) = facts.tmux_anchor.as_ref() else {
            return Ok(());
        };
        crate::client::adapters::tmux::validate_pane_id(&anchor.pane_id)
            .map_err(anyhow::Error::msg)?;
        if !Path::new(&anchor.socket_path).is_absolute()
            || anchor.socket_path.chars().any(char::is_control)
        {
            anyhow::bail!("IDENTITY_FACT_INVALID: invalid tmux socket anchor");
        }
        if let Some(candidate) = facts.tmux.as_ref() {
            if candidate.endpoint.socket_path != anchor.socket_path
                || candidate.endpoint.pane_id != anchor.pane_id
            {
                anyhow::bail!("IDENTITY_FACT_INVALID: tmux candidate differs from stable anchor");
            }
            return Ok(());
        }
        let state = self.host.state.lock().unwrap();
        let mut bindings = state
            .global
            .current_thread_routes
            .values()
            .filter(|binding| {
                binding.tmux_endpoint.as_ref().is_some_and(|endpoint| {
                    endpoint.socket_path == anchor.socket_path && endpoint.pane_id == anchor.pane_id
                })
            });
        let binding = bindings.next().ok_or_else(|| anyhow::anyhow!("IDENTITY_RESTORE_NOT_FOUND: stable pane has no durable binding; live enrichment unavailable"))?;
        if bindings.next().is_some() {
            anyhow::bail!("IDENTITY_RESTORE_CONFLICT: ambiguous stable pane");
        }
        if binding.project_scope != context.project_scope
            || binding.app_scope_id != context.app_scope_id
        {
            anyhow::bail!("IDENTITY_RESTORE_CROSS_PROJECT: stable pane belongs to another scope");
        }
        if facts.session_id.as_deref().is_none_or(|session| {
            binding.session_id.as_ref().map(identity::SessionId::as_str) != Some(session)
        }) {
            anyhow::bail!("IDENTITY_RESTORE_NOT_FOUND: pane alone cannot select an agent identity; provide the current agent session");
        }
        facts.tmux = Some(crate::proto::TmuxCandidate {
            endpoint: binding
                .tmux_endpoint
                .clone()
                .expect("filtered pane binding"),
            cwd: context.canonical_root.clone(),
        });
        Ok(())
    }
    fn approved_recovery_admission(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
    ) -> Result<Option<ApprovedAdmission>, ApprovedAdmissionError> {
        if request.approval.is_none() && request.grant_approval.is_none() {
            return Ok(None);
        }
        if name_adjudication_control(request)?.is_some() {
            let facts = self
                .observe_identity_facts(context, &request.facts)
                .map_err(|error| ApprovedAdmissionError::Error(error.to_string()))?;
            if self
                .current_session_identity(context, &facts)
                .map_err(|error| ApprovedAdmissionError::Error(error.to_string()))?
                .is_none()
            {
                return Err(ApprovedAdmissionError::Error(
                    "NAME_ADJUDICATION_REGISTERED_ACTOR_REQUIRED".into(),
                ));
            }
            return Ok(None);
        }
        if let Some(intent) = name_resolution::parse_name_intent(
            request
                .grant_approval
                .as_ref()
                .or(request.approval.as_ref()),
        )
        .map_err(ApprovedAdmissionError::Error)?
        {
            if !intent.user_authorized {
                return Err(ApprovedAdmissionError::Error(
                    "NAME_AUTHORIZATION_REQUIRED: name restoration needs user authorization".into(),
                ));
            }
            // Name intent does not replace another principal's credential.
            // Its role transition belongs to the typed grant owner after the
            // current caller binding and the name decision are established.
            return Ok(None);
        }
        let observed_facts = self
            .observe_identity_facts(context, &request.facts)
            .map_err(|error| ApprovedAdmissionError::Error(error.to_string()))?;
        if let Some(raw) = request
            .grant_approval
            .as_ref()
            .or(request.approval.as_ref())
            .filter(|raw| {
                raw.get("target_identity").is_none()
                    && matches!(
                        raw["action"].as_str(),
                        Some("restore_master" | "restore_peer")
                    )
            })
        {
            let scope = Scope {
                root: PathBuf::from(&context.canonical_root),
            };
            let mut identity = if let Some(identity) = self
                .current_session_identity(context, &observed_facts)
                .map_err(|error| ApprovedAdmissionError::Error(error.to_string()))?
            {
                identity
            } else {
                identity::resolve_for_daemon_with_route_at(
                    &self.host.host_paths,
                    &scope,
                    &observed_facts,
                    None,
                )
                .map_err(|error| ApprovedAdmissionError::Error(error.to_string()))?
            };
            self.reconcile_committed_credential(&scope, &observed_facts, &mut identity)
                .map_err(|error| ApprovedAdmissionError::Error(error.to_string()))?;
            let mut intent = raw.clone();
            // Legacy role intent expresses approval through action plus the
            // required authorization text. Preserve an explicit denial.
            if intent.get("decision").is_none() {
                intent["decision"] = json!("approved");
            }
            intent["target_identity"] = json!(identity.worker_id);
            let mut identity_approval =
                parse_identity_approval(&intent).map_err(ApprovedAdmissionError::Error)?;
            if (!identity_approval.project_scope.is_empty()
                && identity_approval.project_scope != context.project_scope.as_str())
                || (!identity_approval.app_scope_id.is_empty()
                    && identity_approval.app_scope_id != context.app_scope_id.as_str())
            {
                return Err(ApprovedAdmissionError::Error(
                    "IDENTITY_APPROVAL_SCOPE_MISMATCH".into(),
                ));
            }
            identity_approval.project_scope = context.project_scope.as_str().into();
            identity_approval.app_scope_id = context.app_scope_id.as_str().into();
            identity_approval.decided_by = "user".into();
            identity_approval.approved_at_ms = now_ms();
            identity_approval.expected_incumbent_binding_id.clear();
            identity_approval.expected_incumbent_generation = 0;
            return Ok(Some(ApprovedAdmission {
                identity_approval,
                identity,
                grant_approval: None,
                grant_before: None,
            }));
        }
        let Some(raw_identity) = request.approval.as_ref().or_else(|| {
            request.grant_approval.as_ref().filter(|approval| {
                matches!(
                    approval["action"].as_str(),
                    Some("restore_master" | "restore_peer")
                )
            })
        }) else {
            if request.grant_approval.is_some() {
                return Err(ApprovedAdmissionError::Error(
                    "IDENTITY_APPROVAL_REQUIRED: grant approval cannot replace the incumbent identity approval".into(),
                ));
            }
            return Ok(None);
        };
        let mut identity_approval =
            parse_identity_approval(raw_identity).map_err(ApprovedAdmissionError::Error)?;
        if identity_approval.project_scope.is_empty() {
            identity_approval.project_scope = context.project_scope.as_str().to_owned();
        }
        if identity_approval.app_scope_id.is_empty() {
            identity_approval.app_scope_id = context.app_scope_id.as_str().to_owned();
        }
        if identity_approval.project_scope != context.project_scope.as_str()
            || identity_approval.app_scope_id != context.app_scope_id.as_str()
        {
            return Err(ApprovedAdmissionError::Error(
                "IDENTITY_APPROVAL_SCOPE_MISMATCH".into(),
            ));
        }
        let digest = approval_digest(raw_identity, &request.facts);
        if !identity_approval.intent_digest.is_empty() && identity_approval.intent_digest != digest
        {
            return Err(ApprovedAdmissionError::Error(
                "IDENTITY_APPROVAL_INTENT_MISMATCH".into(),
            ));
        }
        if identity_approval.decided_by.trim().is_empty() {
            identity_approval.decided_by = "user".into();
        }
        if identity_approval.approved_at_ms <= 0 {
            identity_approval.approved_at_ms = crate::server::state::now_ms();
        }
        identity::validate_id_for_protocol(&identity_approval.target_identity).map_err(
            |error| {
                ApprovedAdmissionError::Error(format!("IDENTITY_APPROVAL_TARGET_INVALID: {error}"))
            },
        )?;
        let mut identity =
            identity::read_persisted(&self.host.host_paths, &identity_approval.target_identity)
                .map_err(|error| {
                    ApprovedAdmissionError::Error(format!(
                        "IDENTITY_APPROVAL_IDENTITY_READ_FAILED: {error:#}"
                    ))
                })?
                .ok_or_else(|| {
                    ApprovedAdmissionError::Error("IDENTITY_APPROVAL_IDENTITY_NOT_FOUND".into())
                })?;
        if identity.project_scope.as_ref().is_some_and(|scope| scope != &context.project_scope) {
            return Err(ApprovedAdmissionError::Error(
                "IDENTITY_APPROVAL_SCOPE_MISMATCH".into(),
            ));
        }
        let route_scope = crate::scope::RouteScope {
            app_scope_id: context.app_scope_id.clone(),
            project_scope_id: context.project_scope.clone(),
        };
        let (observed_binding, authoritative_token) = {
            let state = self.host.state.lock().unwrap();
            let observed_binding = observed_facts
                .tmux
                .as_ref()
                .and_then(|tmux| {
                    state
                        .global
                        .lookup_unique_tmux_pane_route(&tmux.endpoint)
                        .filter(|binding| binding.route_scope() == route_scope)
                        .cloned()
                })
                .or_else(|| {
                    identity.runtime.as_ref().and_then(|runtime| {
                        state
                            .global
                            .lookup_binding_for(&route_scope, &runtime.binding_id)
                            .cloned()
                    })
                });
            let authoritative_token = state
                .workers
                .get(&identity.worker_id)
                .map(|worker| worker.token.clone());
            (observed_binding, authoritative_token)
        };
        if let Some(token) = authoritative_token {
            identity.token = token;
        }
        // The project reducer owns a nonresident worker's credential. The
        // host index owns the pane claim, but is not a second worker store.
        if let Ok(runtime) = self.select_runtime(context) {
            if let Some(worker) = runtime
                .state
                .lock()
                .unwrap()
                .workers
                .get(&identity.worker_id)
            {
                identity.token = worker.token.clone();
            }
        }
        if let Some(binding) = observed_binding.as_ref() {
            identity_approval.expected_incumbent_binding_id =
                binding.binding_id.as_str().to_owned();
            identity_approval.expected_incumbent_generation = binding.endpoint_generation;
        } else {
            identity_approval.expected_incumbent_binding_id.clear();
            identity_approval.expected_incumbent_generation = 0;
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
            .filter(|raw| {
                !matches!(
                    raw["action"].as_str(),
                    Some("restore_master" | "restore_peer")
                )
            })
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
        runtime: &Server,
        context: &ProjectContext,
        request: &IdentityContextRequest,
        response: &mut Resp,
        approved: &ApprovedAdmission,
    ) -> Result<(), String> {
        if matches!(
            approved.identity_approval.action.as_str(),
            "restore_master" | "restore_peer"
        ) {
            let approval = &approved.identity_approval;
            let (binding_id, generation) = registered_binding_fence(response)
                .ok_or_else(|| "IDENTITY_ROLE_BINDING_MISSING".to_owned())?;
            let route_scope = crate::scope::RouteScope {
                app_scope_id: context.app_scope_id.clone(),
                project_scope_id: context.project_scope.clone(),
            };
            let mut state = runtime.state.lock().unwrap();
            let binding = state
                .global
                .lookup_binding_for(&route_scope, &binding_id)
                .filter(|binding| {
                    binding.agent_id.as_str() == approval.target_identity
                        && binding.endpoint_generation == generation
                })
                .ok_or_else(|| "IDENTITY_ROLE_BINDING_CHANGED".to_owned())?;
            let worker_id = binding.agent_id.as_str().to_owned();
            let events = if approval.action == "restore_master" {
                let grant = master_grant_for_worker(
                    &state,
                    &route_scope,
                    &worker_id,
                    &approval.decided_by,
                    &approval.authorization,
                )?;
                master_authority_transfer_events(&state, &route_scope, grant)
            } else if current_master_grant(&state, Some(&route_scope))
                .is_some_and(|grant| grant.agent_id.as_str() == worker_id)
            {
                master_authority_revoke_events(&state, &route_scope)
            } else {
                Vec::new()
            };
            self.operation_journal
                .begin_owner_start(&request.operation_id, "grant")?;
            if !events.is_empty() {
                runtime
                    .commit_locked(&mut state, &events)
                    .map_err(|error| format!("MASTER_DURABILITY_FAILED: {error}"))?;
            }
            let is_master = current_master_grant(&state, Some(&route_scope)).is_some_and(|grant| {
                grant.agent_id.as_str() == worker_id
                    && grant.binding_id == binding_id
                    && grant.endpoint_generation == generation
            });
            if is_master != (approval.action == "restore_master") {
                return Err("IDENTITY_ROLE_READBACK_MISMATCH".into());
            }
            drop(state);
            let current: identity::Identity =
                serde_json::from_value(response.data["identity_receipt"].clone())
                    .map_err(|error| format!("IDENTITY_ROLE_RECEIPT_INVALID: {error}"))?;
            let binding = current
                .runtime
                .as_ref()
                .ok_or_else(|| "IDENTITY_ROLE_BINDING_MISSING".to_owned())?;
            let registered =
                ProjectContext::for_registered_route(Path::new(&context.canonical_root), binding)
                    .map_err(|error| error.to_string())?;
            let refreshed = self
                .dispatch_sync(
                    Some(registered),
                    Req::Context {
                        worker_id: current.worker_id,
                        token: current.token,
                    },
                )
                .1;
            if !refreshed.ok {
                return Err(refreshed
                    .error
                    .unwrap_or_else(|| "IDENTITY_ROLE_READBACK_FAILED".into()));
            }
            let name_outcome = response.data["snapshot"]["name_resolution"].clone();
            response.data["snapshot"] = refreshed.data;
            name_resolution::project_registered_name(
                runtime,
                &approval.target_identity,
                &mut response.data["snapshot"],
            );
            response.data["snapshot"]["name_resolution"] = name_outcome;
            return Ok(());
        }
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
        allow_persisted_restore: bool,
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
        facts = self.observe_identity_facts(&context, &facts)?;
        validate_facts(&facts)?;
        let scope = Scope { root };
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
        // A pending project commit is a separate typed recovery path. It must
        // authenticate the stored credential before the ordinary resolver,
        // which continues to reject split host/project bindings.
        let session_identity = if approved_identity.is_some() {
            None
        } else {
            self.current_session_identity(&context, &facts)?
        };
        let staged_identity = if approved_identity.is_some() || session_identity.is_some() {
            None
        } else if let Some(candidate) = facts.tmux.as_ref() {
            let host_binding = self
                .host
                .state
                .lock()
                .unwrap()
                .global
                .lookup_unique_tmux_pane_route(&candidate.endpoint)
                .cloned();
            if let Some(host) = host_binding.as_ref().filter(|binding| {
                facts.session_id.as_deref().is_some_and(|session| {
                    binding.session_id.as_ref().map(identity::SessionId::as_str) == Some(session)
                })
            }) {
                let runtime_context = ProjectContext::for_registered_root_with_app(
                    &scope.root,
                    context.app_scope_id.clone(),
                )?;
                let runtime = self
                    .select_runtime(&runtime_context)
                    .map_err(anyhow::Error::msg)?;
                if let Some(staged_binding) = self
                    .pending_same_pane_master_bindings(&runtime)
                    .into_iter()
                    .find(|binding| {
                        host != binding
                            && binding.tmux_endpoint.as_ref().is_some_and(|endpoint| {
                                crate::client::adapters::tmux::same_pane_route(
                                    endpoint,
                                    &candidate.endpoint,
                                )
                            })
                    })
                {
                    let existing =
                        identity::existing_for_daemon_at(&self.host.host_paths, &scope, &facts)?;
                    let ident = existing.ok_or_else(|| {
                        anyhow::anyhow!(
                            "RECOVERY_RECONCILE_REQUIRED: staged pane has no persisted credential"
                        )
                    })?;
                    let route = self
                        .resolve_staged_pane_recovery(
                            &candidate.endpoint,
                            &ident.worker_id,
                            &ident.token,
                        )
                        .map_err(anyhow::Error::msg)?;
                    if route.project_scope != context.project_scope
                        || route.app_scope_id != context.app_scope_id
                    {
                        return Err(anyhow::anyhow!(
                            "IDENTITY_RESTORE_CONFLICT: staged pane scope differs from context"
                        )
                        .into());
                    }
                    if ident.runtime.as_ref().is_none_or(|runtime| {
                        runtime.agent_id != host.agent_id
                            || runtime.runtime_id != host.runtime_id
                            || runtime.appserver_id != host.app_scope_id
                            || runtime.binding_id != host.binding_id
                            || runtime.endpoint_generation != host.endpoint_generation
                            || runtime.session_id != host.session_id
                            || runtime.native_thread_id != host.native_thread_id
                    }) {
                        return Err(anyhow::anyhow!("IDENTITY_RESTORE_CONFLICT: staged credential binding differs from host").into());
                    }
                    Some((ident, route, runtime, staged_binding))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        let pane_route = if approved_identity.is_some() || session_identity.is_some() {
            None
        } else if let Some((_, route, _, _)) = staged_identity.as_ref() {
            Some(route.clone())
        } else if let Some(candidate) = facts.tmux.as_ref() {
            match self.resolve_route_by_tmux_endpoint(&candidate.endpoint) {
                Ok(route)
                    if facts.session_id.as_deref().is_some_and(|session| {
                        route.session_id.as_ref().map(identity::SessionId::as_str) == Some(session)
                    }) =>
                {
                    Some(route)
                }
                Ok(_) => None,
                // A stale host index, or no route at all, is not usable route
                // evidence: the caller still bootstraps from its live anchors.
                // Every other error stays fatal, so an ambiguity or a real
                // binding conflict still reaches the identity owner.
                Err(error)
                    if error.starts_with("ROUTE_RESOLVE_NOT_FOUND:")
                        || error.starts_with("SESSION_THREAD_BINDING_STALE:")
                        || error.starts_with("ROUTE_RESOLVE_STALE_INDEX:") =>
                {
                    None
                }
                Err(error) => return Err(anyhow::Error::msg(error).into()),
            }
        } else {
            None
        };
        let completing_staged = staged_identity.is_some();
        let staged_owner = staged_identity
            .as_ref()
            .map(|(_, _, runtime, binding)| (runtime.clone(), binding.clone()));
        let approved_move = approved_identity.is_some();
        let mut ident = if let Some(identity) = approved_identity {
            identity
        } else if let Some(identity) = session_identity {
            identity
        } else if let Some((identity, _, _, _)) = staged_identity {
            // The staged proof verified this exact token in the project owner.
            // Equality-only credential refresh would reject the split again.
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
        // A converged durable binding restores its selected adaptor directly.
        // Only an authenticated staged publication or a different approved
        // principal proceeds to Register below.
        if allow_persisted_restore && !completing_staged {
            if let Some(binding) = ident.runtime.clone() {
                let registered_context =
                    ProjectContext::for_registered_route(&scope.root, &binding)?;
                let runtime = self
                    .select_runtime(&registered_context)
                    .map_err(anyhow::Error::msg)?;
                let mut current = {
                    let state = runtime.state.lock().unwrap();
                    let route_scope = crate::scope::RouteScope {
                        app_scope_id: binding.appserver_id.clone(),
                        project_scope_id: context.project_scope.clone(),
                    };
                    state
                        .global
                        .lookup_binding_for(&route_scope, &binding.binding_id)
                        .filter(|saved| {
                            saved.agent_id == binding.agent_id
                                && saved.runtime_id == binding.runtime_id
                                && saved.endpoint_generation == binding.endpoint_generation
                        })
                        .cloned()
                };
                let session_continuation = facts.session_id.as_deref().is_some_and(|session| {
                    current.as_ref().is_some_and(|saved| {
                        saved.session_id.as_ref().map(identity::SessionId::as_str) != Some(session)
                    }) && runtime
                        .state
                        .lock()
                        .unwrap()
                        .name_occupancies
                        .get(context.project_scope.as_str())
                        .is_some_and(|names| {
                            names
                                .values()
                                .any(|holder| holder.worker_id == ident.worker_id)
                        })
                });
                if let (Some(saved), Some(candidate), Some(session)) = (
                    current.as_ref(),
                    facts.tmux.as_ref(),
                    facts.session_id.as_ref(),
                ) {
                    if saved
                        .session_id
                        .as_ref()
                        .is_some_and(|saved| saved.as_str() == session)
                    {
                        current = Some(self.refresh_session_endpoint(
                            &runtime, saved, candidate, &mut ident, &scope,
                        )?);
                    }
                }
                if facts.tmux.is_none() {
                    if let (Some(saved), Some(mut transport)) = (current.as_ref(), ident.transport.clone()) {
                        if facts.desktop_bridge.is_some()
                            && saved.session_id.as_ref().map(identity::SessionId::as_str) == facts.session_id.as_deref()
                        {
                            let candidate = facts.desktop_bridge.as_ref().unwrap();
                            transport = crate::client::adapters::desktop_bridge::verify_candidate(candidate)
                                .map_err(|e| anyhow::anyhow!(e))?;
                            current = Some(self.refresh_session_transport(
                                &runtime, saved, transport, &mut ident, &scope,
                            )?);
                        } else if transport.kind == TransportKind::AppServer
                            && saved.session_id.as_ref().map(identity::SessionId::as_str) == facts.session_id.as_deref()
                        {
                            transport.session_id = facts.session_id.clone();
                            transport.thread_id = facts.thread_id.clone();
                            transport.endpoint = facts.endpoint.clone();
                            transport.namespace = facts.namespace.clone();
                            transport.tmux_endpoint = None;
                            current = Some(self.refresh_session_transport(
                                &runtime, saved, transport, &mut ident, &scope,
                            )?);
                        }
                    }
                }
                let binding = ident.runtime.clone().expect("restored runtime");
                let registered_context =
                    ProjectContext::for_registered_route(&scope.root, &binding)?;
                let host_has_publication = |saved: &RuntimeBinding| {
                    let state = self.host.state.lock().unwrap();
                    match saved.tmux_endpoint.as_ref() {
                        Some(endpoint) => {
                            state.global.lookup_unique_tmux_pane_route(endpoint) == Some(saved)
                        }
                        None => saved
                            .session_id
                            .as_ref()
                            .zip(saved.native_thread_id.as_ref())
                            .is_some_and(|(session, thread)| {
                                state.global.lookup_current_thread_route(session, thread)
                                    == Some(saved)
                            }),
                    }
                };
                let mut published = current
                    .as_ref()
                    .is_some_and(|saved| host_has_publication(saved));
                let anchor_agrees = current.as_ref().is_some_and(|saved| {
                    match (facts.tmux.as_ref(), saved.tmux_endpoint.as_ref()) {
                        (Some(candidate), Some(endpoint)) => {
                            crate::client::adapters::tmux::same_pane_route(
                                &candidate.endpoint,
                                endpoint,
                            )
                        }
                        (Some(_), None) => false,
                        _ => true,
                    }
                });
                if !published && anchor_agrees {
                    // The project journal still owns this durable binding but
                    // the host index lost its publication. Recover the host
                    // route from this exact durable owner. A conflict on an
                    // unrelated project must not block this peer's recovery,
                    // while another claimant on this pane or native address
                    // still fails closed.
                    if let Some(saved) = current.as_ref() {
                        self.reconcile_binding_publication(saved)
                            .map_err(anyhow::Error::msg)?;
                    }
                    published = current
                        .as_ref()
                        .is_some_and(|saved| host_has_publication(saved));
                }
                if published && anchor_agrees && !session_continuation {
                    let (runtime, mut snapshot) = self.dispatch_sync(
                        Some(registered_context),
                        Req::Context {
                            worker_id: ident.worker_id.clone(),
                            token: ident.token.clone(),
                        },
                    );
                    if !snapshot.ok {
                        return Ok((runtime, snapshot, None));
                    }
                    add_optional_master_choice(&mut snapshot.data);
                    let persisted =
                        identity::read_persisted(&self.host.host_paths, &ident.worker_id)?;
                    if persisted.as_ref().map(|saved| json!(saved)) != Some(json!(ident)) {
                        let transport = ident.transport.clone().ok_or_else(|| {
                            anyhow::anyhow!(
                                "IDENTITY_RESTORE_CONFLICT: persisted selected adaptor missing"
                            )
                        })?;
                        identity::persist_registration_at(
                            &self.host.host_paths,
                            &scope,
                            &mut ident,
                            binding,
                            transport,
                        )?;
                    }
                    return Ok((
                        runtime,
                        Resp::data(json!({"snapshot":snapshot.data,"identity_receipt":ident})),
                        None,
                    ));
                }
                if !session_continuation && (!approved_move || anchor_agrees) {
                    let retired = {
                        let state = runtime.state.lock().unwrap();
                        !state.workers.contains_key(&ident.worker_id)
                    };
                    if retired {
                        // An archive cannot establish a current registration.
                        // This includes old epochs without WorkerClosed events.
                        // Re-register from the live session facts while keeping
                        // the exact archived credential and name continuity.
                        ident.runtime = None;
                        ident.transport = None;
                    } else {
                        return Err(anyhow::anyhow!("RECOVERY_RECONCILE_REQUIRED: durable identity binding is not published at its saved generation").into());
                    }
                }
            }
        }
        let (runtime, receipt, staged_registration) = if let Some((runtime, binding)) = staged_owner
        {
            let (committed, transport) =
                committed_register_readback(&runtime, &binding, &ident.worker_id, &ident.token)
                    .map_err(anyhow::Error::msg)?;
            if let Some(outer) = outer_binding.as_ref() {
                outer
                    .bind_committed_receipt(&committed)
                    .map_err(anyhow::Error::msg)?;
                let completion_start = outer
                    .arbitrate_owner_start()
                    .and_then(|()| outer.promote_inner_dispatched());
                // There is no new Register consume to take this TLS hook.
                clear_context_cancel_operation();
                completion_start.map_err(anyhow::Error::msg)?;
            }
            let staged_runtime = RuntimeIdentity {
                agent_id: binding.agent_id.clone(),
                runtime_id: binding.runtime_id.clone(),
                appserver_id: binding.app_scope_id.clone(),
                endpoint_generation: binding.endpoint_generation,
                binding_id: binding.binding_id.clone(),
                session_id: binding.session_id.clone(),
                native_thread_id: binding.native_thread_id.clone(),
            };
            let registered_context =
                ProjectContext::for_registered_route(&scope.root, &staged_runtime)?;
            let previous = self
                .registration_rollback_state(&runtime, &registered_context, &ident.worker_id)
                .map_err(anyhow::Error::msg)?;
            if previous.0.as_ref() != Some(&binding) {
                return Err(anyhow::anyhow!(
                    "RECOVERY_RECONCILE_REQUIRED: staged owner changed before publication"
                )
                .into());
            }
            let readback = Resp::data(json!({
                "worker_id": ident.worker_id,
                "transport_selected": transport,
                "typed": true,
                "replayed": true,
                "command_id": committed.command_id,
                "operation_id": committed.operation_id,
                "sequence": committed.sequence,
                "revision": committed.revision,
                "binding": binding,
            }));
            let (runtime, receipt) = self.finalize_registration(
                runtime,
                &registered_context,
                Some(&ident.worker_id),
                previous.0,
                previous.1,
                previous.2,
                previous.3,
                readback,
            );
            (runtime, receipt, Some((staged_runtime, transport)))
        } else {
            if facts.dsh_session_id.is_some() && facts.dsh.is_none() && facts.tmux.is_none() {
                return Err(anyhow::anyhow!("IDENTITY_INFORMATION_REQUIRED: first DSH registration requires dsh endpoint, runtime_id, agent_id, session_id and cwd facts").into());
            }
            let candidates = TransportCandidates {
                desktop_bridge: facts.desktop_bridge,
                // A pane needs no native IDs. Only construct the adjacent
                // candidate when all its facts exist; selected native paths
                // already require these fields in preflight.
                appserver: match (
                    &facts.endpoint,
                    &facts.namespace,
                    &facts.session_id,
                    &facts.thread_id,
                ) {
                    (Some(endpoint), Some(namespace), Some(session_id), Some(thread_id)) => {
                        Some(AppServerCandidate {
                            endpoint: endpoint.clone(),
                            namespace: namespace.clone(),
                            session_id: session_id.clone(),
                            thread_id: thread_id.clone(),
                            cwd: context.canonical_root.clone(),
                        })
                    }
                    _ => None,
                },
                tmux: facts.tmux,
                dsh: facts.dsh,
            };
            let provisional = RuntimeIdentity::cli_adapter(&ident.worker_id)?;
            let register_runtime = ident.runtime.as_ref().unwrap_or(&provisional);
            let register_context =
                ProjectContext::for_registered_route(&scope.root, register_runtime)?;
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
            (runtime, receipt, None)
        };
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
        let (binding, transport) = match staged_registration {
            Some(registration) => registration,
            None => {
                identity::registration_from_receipt(&receipt.data, &ident.worker_id, &scope.root)
                    .map_err(post_register)?
            }
        };
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
        let (runtime, mut snapshot) = self.dispatch_sync(
            Some(registered_context),
            Req::Context {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
            },
        );
        if !snapshot.ok {
            return Ok((runtime, snapshot, Some(nested)));
        }
        add_optional_master_choice(&mut snapshot.data);
        Ok((
            runtime,
            Resp::data(json!({
                "snapshot": snapshot.data,
                "identity_receipt": ident
            })),
            Some(nested),
        ))
    }

    /// A committed registration can outlive its local receipt file. Recover
    /// that same credential from the reducer only for the proven anchor.
    /// A unique current pane owner supplies the authoritative project credential
    /// even when its local receipt is stale. Ordinary token admission is unchanged.
    fn reconcile_committed_credential(
        &self,
        scope: &Scope,
        facts: &IdentityFacts,
        ident: &mut identity::Identity,
    ) -> anyhow::Result<()> {
        let current_context = ProjectContext::for_registered_root_with_app(
            &scope.root,
            AppServerId::new(identity::CLI_APP_SERVER_ID)?,
        )?;
        let host_binding = self.current_anchor_binding(&current_context, facts)?;
        let authoritative =
            if let Some(binding) = host_binding.as_ref().filter(|_| facts.tmux.is_some()) {
                let context = ProjectContext::for_registered_root_with_app(
                    &scope.root,
                    binding.app_scope_id.clone(),
                )?;
                let runtime = self.select_runtime(&context).map_err(anyhow::Error::msg)?;
                let state = runtime.state.lock().unwrap();
                (|| {
                    if binding.project_scope.as_str() != scope.root.to_string_lossy()
                        || binding.agent_id.as_str() != ident.worker_id
                        || state
                            .global
                            .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
                            != Some(binding)
                    {
                        return None;
                    }
                    let worker = state.workers.get(&ident.worker_id)?;
                    let transport = worker.transport.clone()?;
                    Some(identity::Identity {
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
                    })
                })()
            } else {
                None
            };
        if let Some(authoritative) = authoritative {
            *ident = authoritative;
            return Ok(());
        }
        let committed_locally = identity::read_persisted(&self.host.host_paths, &ident.worker_id)?
            .is_some_and(|persisted| persisted.runtime.is_some());
        if ident.runtime.is_some() || committed_locally {
            return Ok(());
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
                let anchor_matches = binding.session_id.as_ref().map(identity::SessionId::as_str)
                    == facts.session_id.as_deref()
                    && facts.thread_id.as_deref().is_none_or(|thread| {
                        binding
                            .native_thread_id
                            .as_ref()
                            .is_some_and(|saved| saved.as_str() == thread)
                    });
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

/// Typed name occupancy for the current session in this project. It is the
/// only owner evidence for a multi-binding current session; mtime, archive
/// order, pane history and subjective liveness never select a peer.
fn typed_session_owner(
    state: &crate::server::state::State,
    project_scope: &str,
    session_id: &str,
) -> anyhow::Result<Option<String>> {
    let Some(names) = state.name_occupancies.get(project_scope) else {
        return Ok(None);
    };
    let matches = names
        .values()
        .filter(|holder| {
            holder.session_id == session_id && state.workers.contains_key(holder.worker_id.as_str())
        })
        .map(|holder| holder.worker_id.clone())
        .collect::<Vec<_>>();
    Ok(if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    })
}
