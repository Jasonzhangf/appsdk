impl ProjectRuntimeManager {
    pub(super) fn recovered_binding_identity(
        &self,
        context: &ProjectContext,
        binding: &RuntimeBinding,
        state: &crate::server::state::State,
    ) -> Option<identity::Identity> {
        let worker = state.workers.get(binding.agent_id.as_str())?;
        let mut transport = worker.transport.clone();
        if let (Some(transport), Some(endpoint)) =
            (transport.as_mut(), binding.tmux_endpoint.as_ref())
        {
            if transport.kind == TransportKind::Tmux {
                transport.endpoint = Some(endpoint.socket_path.clone());
                transport.namespace = Some(endpoint.tmux_session_id.clone());
                transport.session_id = binding.session_id.as_ref().map(|id| id.as_str().to_owned());
                transport.thread_id = binding
                    .native_thread_id
                    .as_ref()
                    .map(|id| id.as_str().to_owned());
                transport.tmux_endpoint = Some(endpoint.clone());
            }
        }
        Some(identity::Identity {
            worker_id: worker.id.clone(),
            token: worker.token.clone(),
            project_scope: Some(context.project_scope.clone()),
            runtime: Some(RuntimeIdentity {
                agent_id: binding.agent_id.clone(),
                runtime_id: binding.runtime_id.clone(),
                appserver_id: binding.app_scope_id.clone(),
                binding_id: binding.binding_id.clone(),
                session_id: binding.session_id.clone(),
                native_thread_id: binding.native_thread_id.clone(),
            }),
            transport,
        })
    }

    /// Enroll or restore the selected gateway's canonical Poll loop for a
    /// DSH transport. Other transports are untouched. The binding handed over
    /// is the already-committed one; this call never registers the identity
    /// again and never echoes the credential outside the control socket.
    fn enroll_dsh_receive_loop(
        &self,
        transport: &crate::proto::SelectedTransport,
        worker_id: &str,
        token: &str,
        project_scope: &str,
        runtime_context: crate::client::adapters::dsh::DshRuntimeContext<'_>,
    ) -> anyhow::Result<()> {
        if transport.kind != crate::proto::TransportKind::Dsh {
            return Ok(());
        }
        let agent_id = runtime_context.native_thread_id.ok_or_else(|| {
            anyhow::anyhow!("DSH_ENDPOINT_REJECTED: committed dsh binding has no gateway agent id")
        })?;
        let endpoint = transport.endpoint.as_deref().ok_or_else(|| {
            anyhow::anyhow!("DSH_ENDPOINT_REJECTED: selected dsh transport has no gateway endpoint")
        })?;
        let runtime_id = transport.namespace.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "DSH_ENDPOINT_REJECTED: selected dsh transport has no gateway runtime id"
            )
        })?;
        crate::client::adapters::dsh::enroll_receive_loop(
            endpoint,
            &crate::client::adapters::dsh::DshLoopEnrollment {
                runtime_id,
                agent_id,
                worker_id,
                token,
                project_scope,
                runtime_context,
            },
        )
        .map_err(|error| {
            let detail = error.to_string();
            let detail = detail
                .strip_prefix("DSH_ENDPOINT_REJECTED: ")
                .unwrap_or(&detail)
                .to_owned();
            anyhow::Error::new(error).context(format!(
                "DSH_ENDPOINT_REJECTED: gateway receive-loop enrollment failed: {detail}"
            ))
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
        self.observe_identity_facts_with_pane(context, facts)
            .map(|(facts, _)| facts)
    }

    /// Return the live caller pane separately from the restored facts. Only
    /// caller ancestry just observed here may populate the name pane anchor;
    /// persisted transport, caller-supplied tmux candidates and pane anchors
    /// completed from durable routes cannot.
    fn observe_identity_facts_with_pane(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
    ) -> anyhow::Result<(IdentityFacts, Option<crate::proto::TmuxPaneAnchor>)> {
        self.observe_identity_facts_from(context, facts)
    }

    fn observe_identity_facts_from(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
    ) -> anyhow::Result<(IdentityFacts, Option<crate::proto::TmuxPaneAnchor>)> {
        let mut facts = facts.clone();
        if facts.rex.is_some() || facts.dsh.is_some() || facts.dsh_session_id.is_some() {
            // DSH owns its gateway anchor. The process may still be launched
            // from a tmux pane, but that ancestry is observation only and
            // cannot become a competing transport candidate.
            facts.tmux = None;
            facts.tmux_anchor = None;
            facts.tmux_session_name = None;
            facts.caller_pid = None;
            facts.pane_hint = None;
            return Ok((facts, None));
        }
        let Some(caller_pid) = facts.caller_pid else {
            // Placement is accepted only from this invocation's live observation.
            facts.tmux = None;
            facts.tmux_anchor = None;
            facts.pane_hint = None;
            facts.tmux_session_name = None;
            return Ok((facts, None));
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
        // tmux placement is optional, but a live pane without its fixed session
        // name is a missing name fact. Fail closed instead of guessing from the
        // agent session or caller-controlled environment.
        let observed = crate::client::adapters::tmux::observe_caller(
            endpoints,
            caller_pid,
            facts.pane_hint.as_deref(),
        )
        .unwrap_or_else(|_error| None);
        // Current ancestry supersedes every caller-supplied or historical
        // placement address. No observation means no tmux selection.
        let observed_pane = observed
            .as_ref()
            .map(|endpoint| crate::proto::TmuxPaneAnchor {
                socket_path: endpoint.socket_path.clone(),
                pane_id: endpoint.pane_id.clone(),
            });
        facts.tmux_session_name = observed.as_ref().and_then(|endpoint| {
            crate::client::adapters::tmux::observe_session_name(endpoint).ok()
        });
        if observed.is_some() && facts.tmux_session_name.is_none() {
            return Err(anyhow::anyhow!(
                "IDENTITY_TMUX_SESSION_NAME_REQUIRED: the daemon observed a tmux pane \
                 but could not read its fixed session name; refusing to register or recover by name"
            ));
        }
        if let Some((kind, profile)) = observed.as_ref().and_then(|endpoint| {
            crate::host_runtime::caller_profile_in_pane(caller_pid, endpoint.pane_pid)
        }) {
            facts.agent_kind = Some(
                match kind {
                    crate::host_runtime::AgentKind::Codex => "codex",
                    crate::host_runtime::AgentKind::Antigravity => "antigravity",
                }
                .into(),
            );
            facts.profile = profile;
        }
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
        Ok((facts, observed_pane))
    }

    fn current_session_identity(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
    ) -> anyhow::Result<Option<identity::Identity>> {
        self.current_session_identity_at_cwd(context, facts, None)
    }

    fn current_session_identity_at_cwd(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
        invocation_cwd: Option<&str>,
    ) -> anyhow::Result<Option<identity::Identity>> {
        let control = self
            .binding_control_store()
            .load()
            .map_err(anyhow::Error::msg)?;
        let Some(slot) = control
            .lookup(
                &context.project_scope,
                &context.app_scope_id,
                facts,
                invocation_cwd,
            )
            .map_err(anyhow::Error::msg)?
        else {
            return Ok(None);
        };
        let value = slot
            .active_binding()
            .expect("lookup returns active current");
        if facts
            .dsh
            .as_ref()
            .is_some_and(|candidate| !identity::dsh_transport_matches(candidate, &value.transport))
        {
            anyhow::bail!("IDENTITY_RESTORE_CONFLICT: DSH communication API differs from the current mapped binding");
        }
        let runtime = self.select_runtime(context).map_err(anyhow::Error::msg)?;
        let state = runtime.state.lock().unwrap();
        let mut ident = self
            .recovered_binding_identity(context, &value.binding, &state)
            .ok_or_else(|| {
                anyhow::anyhow!("IDENTITY_CREDENTIAL_MISSING: mapped worker is not registered")
            })?;
        ident.transport = Some(value.transport.clone());
        Ok(Some(ident))
    }

    pub(super) fn binding_publication_matches(&self, binding: &RuntimeBinding) -> bool {
        let state = self.host.state.lock().unwrap();
        state.global.current_thread_routes.values().any(|current| current == binding)
    }

    fn refresh_session_endpoint(
        &self,
        runtime: &Server,
        saved: &RuntimeBinding,
        candidate: &crate::proto::TmuxCandidate,
        ident: &mut identity::Identity,
        scope: &Scope,
    ) -> anyhow::Result<RuntimeBinding> {
        let mut transport = runtime
            .state
            .lock()
            .unwrap()
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
        let _register = self.register_gate.lock().unwrap();
        // The resident runtime can be the host itself. Read publication before
        // taking its state lock so the shared mutex is never acquired twice.
        let published = self.binding_publication_matches(saved);
        let mut state = runtime.state.lock().unwrap();
        let mut worker = state
            .workers
            .get(&ident.worker_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_WORKER_MISSING"))?;
        if state
            .global
            .lookup_binding_for(&saved.route_scope(), &saved.binding_id)
            != Some(saved)
        {
            anyhow::bail!("IDENTITY_MAP_BINDING_CHANGED: exact binding changed before refresh");
        }
        let same_placement = saved.tmux_endpoint == transport.tmux_endpoint
            && saved.session_id.as_ref().map(identity::SessionId::as_str)
                == transport.session_id.as_deref()
            && saved.native_thread_id.as_ref().map(NativeThreadId::as_str)
                == transport.thread_id.as_deref()
            && (transport.kind == TransportKind::Tmux
                || worker.transport.as_ref().is_some_and(|old| {
                    old.kind == transport.kind
                        && old.endpoint == transport.endpoint
                        && old.namespace == transport.namespace
                }));
        if same_placement
            && published
            && worker.transport.as_ref() == Some(&transport)
            && ident.transport.as_ref() == Some(&transport)
        {
            return Ok(saved.clone());
        }

        let mut next = saved.clone();
        next.tmux_endpoint = transport.tmux_endpoint.clone();
        next.session_id = transport
            .session_id
            .clone()
            .map(identity::SessionId::new)
            .transpose()?;
        next.native_thread_id = transport
            .thread_id
            .clone()
            .map(NativeThreadId::new)
            .transpose()?;
        let control_context =
            ProjectContext::for_registered_root_with_app(&scope.root, saved.app_scope_id.clone())?;
        let key = self
            .mapped_key_for_worker(&control_context, &ident.worker_id, saved)
            .map_err(anyhow::Error::msg)?;
        let session = next
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_SESSION_MISSING"))?;
        let holder = state.name_occupancy(control_context.project_scope.as_str(), &key.name);
        let effect = crate::server::binding_slot::PendingEffect {
            kind: crate::server::binding_slot::PendingEffectKind::Name,
            role: None,
            effect_id: format!("transport-refresh-{}-{}", next.binding_id, now_ms()),
            operation_id: format!("transport-refresh-{}", next.binding_id),
            project_scope: key.project_scope.clone(),
            app_scope_id: key.app_scope_id.clone(),
            name: key.name.clone(),
            previous_worker_id: Some(saved.agent_id.to_string()),
            previous_session_id: saved.session_id.as_ref().map(ToString::to_string),
            target_worker_id: ident.worker_id.clone(),
            target_session_id: session.to_string(),
            occupancy_revision: holder
                .filter(|holder| holder.session_id == session.as_str())
                .map(|holder| holder.occupancy_revision)
                .unwrap_or_else(|| {
                    state.name_occupancy_revision(key.project_scope.as_str(), &key.name) + 1
                }),
            source_names: Vec::new(),
            moves_existing_names: false,
            pane_anchor: holder.and_then(|holder| holder.pane_anchor.clone()),
            created_ms: now_ms(),
        };
        let control = self.binding_control_store().load().map_err(anyhow::Error::msg)?;
        let mut value = crate::server::binding_slot::BindingValue::new(next.clone(), transport.clone());
        Self::prepare_control_value(&control, &key, &mut value).map_err(anyhow::Error::msg)?;
        let rename = crate::server::identity_context::rename_prepared_anchor(
            &value,
            &value.invocation_cwd, &key.name,
            current_master_worker_id(&state, Some(&saved.route_scope())).as_deref() == Some(ident.worker_id.as_str()),
            false,
            None,
        ).map_err(anyhow::Error::msg)?;
        let committed = match self.commit_control_change_locked(
            crate::server::binding_store::ControlChange::CommitWithEffect {
                key, value, expected_generation: None, effect: effect.clone(),
            },
        ) {
            Ok(committed) => committed,
            Err(error) => {
                let error = match rename.map(|rename| rename.rollback()).transpose() {
                    Ok(_) => error,
                    Err(rollback) => format!("{error}; TMUX_SESSION_RENAME_ROLLBACK_FAILED: {rollback}"),
                };
                return Err(anyhow::Error::msg(error));
            }
        };
        if committed.durability == crate::server::binding_store::Durability::Unknown {
            anyhow::bail!("BINDING_CONTROL_DURABILITY_UNKNOWN: binding intent published; business effects remain pending");
        }
        drop(state);
        self.recover_pending_effect_locked(
            &self
                .binding_control_store()
                .load()
                .map_err(anyhow::Error::msg)?,
            &effect,
        )
        .map_err(anyhow::Error::msg)?;
        *ident = identity::read_persisted(&self.host.host_paths, &ident.worker_id)?
            .ok_or_else(|| anyhow::anyhow!("PENDING_EFFECT_CREDENTIAL_MISSING"))?;
        Ok(next)
    }
    fn reconcile_identity_context(
        &self,
        context: ProjectContext,
        mut facts: IdentityFacts,
        outer_binding: Option<RegisterOuterBinding>,
        allow_persisted_restore: bool,
        invocation_cwd: Option<&str>,
    ) -> anyhow::Result<(Arc<Server>, Resp)> {
        if context.app_scope_id.as_str() != identity::CLI_APP_SERVER_ID
            || context.runtime_context.is_some()
        {
            anyhow::bail!(
                "IDENTITY_CONTEXT_INVALID: bootstrap requires the CLI project scope without a caller-selected identity"
            );
        }
        let root = PathBuf::from(&context.canonical_root);
        context.validate_registered_root(&root)?;
        validate_project_registration_cwd(&context.canonical_root, &root)
            .map_err(anyhow::Error::msg)?;
        // The caller has already performed the single live observation and
        // cleared caller_pid/pane_hint. Do not observe a second time. The pane
        // anchor is derived only from that invocation's live observation by
        // `resolve_context_name`; persisted or caller-supplied facts.tmux is
        // never used as a live pane here.
        validate_facts(&facts)?;
        let scope = Scope { root };
        let session_identity =
            self.current_session_identity_at_cwd(&context, &facts, invocation_cwd)?;
        let required =
            if session_identity.is_some() || self.has_current_desktop_binding(&context, &facts)? {
                Vec::new()
            } else {
                required_fields(&facts)
            };
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
            ));
        }
        let mut ident = if let Some(identity) = session_identity {
            identity
        } else {
            identity::new_registration_identity(&scope, &facts)?
        };
        // A converged durable binding restores its selected adaptor directly.
        // Only an authenticated staged publication or a different approved
        // principal proceeds to Register below.
        if allow_persisted_restore {
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
                        })
                        .cloned()
                };
                if let (Some(saved), Some(candidate)) = (current.as_ref(), facts.tmux.as_ref()) {
                    current = Some(self.refresh_session_endpoint(
                        &runtime, saved, candidate, &mut ident, &scope,
                    )?);
                }
                if facts.tmux.is_none() {
                    if let (Some(saved), Some(mut transport)) =
                        (current.as_ref(), ident.transport.clone())
                    {
                        if let Some(candidate) = facts
                            .dsh
                            .as_ref()
                            .filter(|_| saved.native_thread_id.is_none())
                        {
                            let candidates = TransportCandidates {
                                dsh: Some(candidate.clone()),
                                ..Default::default()
                            };
                            transport = validate_transport_candidates(
                                &runtime,
                                &candidates,
                                &context.canonical_root,
                            )
                            .map_err(anyhow::Error::msg)?;
                            current = Some(self.refresh_session_transport(
                                &runtime, saved, transport, &mut ident, &scope,
                            )?);
                        } else if let Some(candidate) = facts.rex.as_ref() {
                            let candidates = TransportCandidates { rex: Some(candidate.clone()), ..Default::default() };
                            transport = validate_transport_candidates_for_session(
                                &runtime, &candidates, &context.canonical_root, facts.session_id.as_deref(),
                            ).map_err(anyhow::Error::msg)?;
                            current = Some(self.refresh_session_transport(
                                &runtime, saved, transport, &mut ident, &scope,
                            )?);
                        } else if facts.desktop_bridge.is_some()
                            && saved.session_id.as_ref().map(identity::SessionId::as_str)
                                == facts.session_id.as_deref()
                        {
                            let candidate = facts.desktop_bridge.as_ref().unwrap();
                            transport = crate::client::adapters::desktop_bridge::verify_candidate(
                                candidate,
                            )
                            .map_err(|e| anyhow::anyhow!(e))?;
                            current = Some(self.refresh_session_transport(
                                &runtime, saved, transport, &mut ident, &scope,
                            )?);
                        } else if transport.kind == TransportKind::AppServer
                            && saved.session_id.as_ref().map(identity::SessionId::as_str)
                                == facts.session_id.as_deref()
                        {
                            transport.session_id = facts.session_id.clone();
                            if facts.thread_id.is_some() {
                                transport.thread_id = facts.thread_id.clone();
                            }
                            if facts.endpoint.is_some() {
                                transport.endpoint = facts.endpoint.clone();
                                transport.namespace = facts.namespace.clone();
                            }
                            transport.tmux_endpoint = None;
                            current = Some(self.refresh_session_transport(
                                &runtime, saved, transport, &mut ident, &scope,
                            )?);
                        } else if transport.kind == TransportKind::Tmux
                            && saved.session_id.as_ref().map(identity::SessionId::as_str)
                                == facts.session_id.as_deref()
                            && self.binding_publication_matches(saved)
                        {
                            // Session recovery may lack a live pane. Its exact
                            // published binding still owns the saved transport
                            // projection, but does not authorize a new claim.
                            current = Some(self.refresh_session_transport(
                                &runtime, saved, transport, &mut ident, &scope,
                            )?);
                        }
                    }
                }
                let binding = ident.runtime.clone().expect("restored runtime");
                let registered_context =
                    ProjectContext::for_registered_route(&scope.root, &binding)?;
                let mut published = current
                    .as_ref()
                    .is_some_and(|saved| self.binding_publication_matches(saved));
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
                    // The current project state still owns this binding but
                    // the host index lost its publication. Recover the host
                    // route from this exact durable owner. A conflict on an
                    // unrelated project must not block this peer's recovery.
                    if let Some(saved) = current.as_ref() {
                        self.reconcile_binding_publication(saved)
                            .map_err(|error| anyhow::anyhow!(error))?;
                    }
                    published = current
                        .as_ref()
                        .is_some_and(|saved| self.binding_publication_matches(saved));
                }
                if published && anchor_agrees {
                    let (runtime, mut snapshot) = self.dispatch_sync(
                        Some(registered_context),
                        Req::Context {
                            worker_id: ident.worker_id.clone(),
                            token: ident.token.clone(),
                        },
                    );
                    if !snapshot.ok {
                        return Ok((runtime, snapshot));
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
                        identity::persist_current_binding_at(
                            &self.host.host_paths,
                            &scope,
                            &mut ident,
                            binding,
                            transport,
                        )?;
                    }
                    if let (Some(saved), Some(transport)) =
                        (current.as_ref(), ident.transport.as_ref())
                    {
                        self.enroll_dsh_receive_loop(
                            transport,
                            ident.worker_id.as_str(),
                            ident.token.as_str(),
                            saved.project_scope.as_str(),
                            crate::client::adapters::dsh::DshRuntimeContext {
                                agent_id: saved.agent_id.as_str(),
                                runtime_id: saved.runtime_id.as_str(),
                                appserver_id: saved.app_scope_id.as_str(),
                                binding_id: saved.binding_id.as_str(),
                                session_id: saved.session_id.as_ref().map(|id| id.as_str()),
                                native_thread_id: saved
                                    .native_thread_id
                                    .as_ref()
                                    .map(|id| id.as_str()),
                            },
                        )?;
                    }
                    return Ok((
                        runtime,
                        Resp::data(json!({"snapshot":snapshot.data,"identity_receipt":ident})),
                    ));
                }
                {
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
        let (runtime, receipt) = {
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
                rex: facts.rex,
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
            (runtime, receipt)
        };
        if !receipt.ok {
            return Ok((runtime, receipt));
        }
        let (binding, transport) =
            identity::registration_from_receipt(&receipt.data, &ident.worker_id, &scope.root)?;
        let registered_context = ProjectContext::for_registered_route(&scope.root, &binding)?;
        identity::persist_current_binding_at(
            &self.host.host_paths,
            &scope,
            &mut ident,
            binding.clone(),
            transport.clone(),
        )?;
        self.enroll_dsh_receive_loop(
            &transport,
            ident.worker_id.as_str(),
            ident.token.as_str(),
            registered_context.project_scope.as_str(),
            crate::client::adapters::dsh::DshRuntimeContext {
                agent_id: binding.agent_id.as_str(),
                runtime_id: binding.runtime_id.as_str(),
                appserver_id: binding.appserver_id.as_str(),
                binding_id: binding.binding_id.as_str(),
                session_id: binding.session_id.as_ref().map(|id| id.as_str()),
                native_thread_id: binding.native_thread_id.as_ref().map(|id| id.as_str()),
            },
        )?;
        let (runtime, mut snapshot) = self.dispatch_sync(
            Some(registered_context),
            Req::Context {
                worker_id: ident.worker_id.clone(),
                token: ident.token.clone(),
            },
        );
        if !snapshot.ok {
            return Ok((runtime, snapshot));
        }
        add_optional_master_choice(&mut snapshot.data);
        Ok((
            runtime,
            Resp::data(json!({
                "snapshot": snapshot.data,
                "identity_receipt": ident
            })),
        ))
    }
}

#[cfg(test)]
mod anchor_provenance_tests {
    use super::*;
    use crate::proto::{TmuxCandidate, TmuxPaneAnchor};
    use crate::server::peer_tests::test_server;
    use std::sync::Arc;

    fn recovery_manager(server: Arc<Server>) -> Arc<ProjectRuntimeManager> {
        let host_paths = server.host_paths.clone();
        ProjectRuntimeManager::new(server, &host_paths).unwrap()
    }

    fn recovery_context(root: &std::path::Path) -> ProjectContext {
        ProjectContext::for_registered_root_with_app(
            root,
            AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap(),
        )
        .unwrap()
    }

    fn tmux_anchor() -> TmuxCandidate {
        TmuxCandidate {
            endpoint: crate::proto::TmuxEndpoint {
                socket_path: "/tmp/tmux-test.sock".into(),
                server_pid: 42,
                tmux_session_id: "$7".into(),
                pane_id: "%3".into(),
                pane_pid: 99,
                codex_session_id: Some("saved-agent-session".into()),
                codex_thread_id: Some("saved-agent-thread".into()),
            },
            cwd: "/tmp/project".into(),
        }
    }

    #[test]
    fn caller_supplied_tmux_candidate_without_live_observation_is_cleared() {
        let (host, root) = test_server();
        let manager = recovery_manager(Arc::new(host));
        let context = recovery_context(&root);
        let candidate = tmux_anchor();
        let facts = IdentityFacts {
            caller_pid: None,
            session_id: Some("caller-claimed-session".into()),
            pane_hint: Some("%3".into()),
            tmux_anchor: Some(TmuxPaneAnchor {
                socket_path: candidate.endpoint.socket_path.clone(),
                pane_id: candidate.endpoint.pane_id.clone(),
            }),
            tmux: Some(candidate),
            tmux_session_name: Some("Alice".into()),
            ..IdentityFacts::default()
        };

        let (observed, pane_anchor) = manager
            .observe_identity_facts_with_pane(&context, &facts)
            .unwrap();

        assert!(
            observed.tmux.is_none(),
            "an unverified caller tmux candidate must not reach identity staging"
        );
        assert!(
            observed.tmux_anchor.is_none(),
            "an unverified caller pane anchor must not survive observation"
        );
        assert!(observed.tmux_session_name.is_none());
        assert!(observed.pane_hint.is_none());
        assert!(
            pane_anchor.is_none(),
            "no live pane observation may produce a name anchor from request facts"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
