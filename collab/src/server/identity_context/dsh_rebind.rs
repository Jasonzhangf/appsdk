#[derive(Clone, Copy)]
enum DshConvergence {
    Endpoint,
    ReceiveLoop,
}

fn dsh_control_error(error: &anyhow::Error) -> Option<&crate::client::adapters::dsh::ControlError> {
    error.chain().find_map(|cause| cause.downcast_ref())
}

impl ProjectRuntimeManager {
    /// Use the current host publication, rather than ranking historical
    /// bindings by generation or lexical binding ID.
    pub(super) fn current_adapter_bindings(
        &self,
        runtime: &Arc<Server>,
        kind: &TransportKind,
    ) -> Vec<RuntimeBinding> {
        let state = runtime.state.lock().unwrap();
        let candidates = state
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .filter(|binding| {
                state
                    .workers
                    .get(binding.agent_id.as_str())
                    .and_then(|worker| worker.transport.as_ref())
                    .is_some_and(|transport| &transport.kind == kind)
            })
            .cloned()
            .collect::<Vec<_>>();
        drop(state);
        let host = self.host.state.lock().unwrap();
        candidates
            .into_iter()
            .filter(|binding| {
                binding
                    .session_id
                    .as_ref()
                    .zip(binding.native_thread_id.as_ref())
                    .is_some_and(|(session, thread)| {
                        host.global.lookup_current_thread_route(session, thread) == Some(binding)
                    })
            })
            .collect()
    }

    pub(super) fn reconcile_dsh_binding(
        &self,
        runtime: &Arc<Server>,
        saved: &RuntimeBinding,
    ) -> anyhow::Result<()> {
        self.converge_dsh_binding(runtime, saved, DshConvergence::ReceiveLoop)
    }

    /// An explicit `unknown-agent` answer is terminal: the gateway does not know
    /// the committed agent at all. Retire the current route and advance the
    /// binding generation with no thread/pane, but keep the worker record so
    /// name ownership, tasks, and mailbox history stay intact.
    fn retire_absent_dsh_binding(
        &self,
        runtime: &Arc<Server>,
        saved: &RuntimeBinding,
    ) -> anyhow::Result<()> {
        let _guard = self.identity_gate.lock().unwrap();
        let mut state = runtime.state.lock().unwrap();
        if state
            .global
            .lookup_binding_for(&saved.route_scope(), &saved.binding_id)
            != Some(saved)
        {
            return Ok(());
        }
        let next_generation = saved
            .endpoint_generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_ENDPOINT_GENERATION_EXHAUSTED"))?;
        let mut retired = saved.clone();
        retired.endpoint_generation = next_generation;
        retired.native_thread_id = None;
        retired.tmux_endpoint = None;
        let events = [
            Event::GlobalCurrentThreadRouteRetired {
                binding: saved.clone(),
            },
            Event::GlobalRuntimeBound { binding: retired },
        ];
        runtime.commit_locked(&mut state, &events)?;
        drop(state);
        if !Arc::ptr_eq(runtime, &self.host) {
            self.host.commit_checked(&[Event::GlobalCurrentThreadRouteRetired {
                binding: saved.clone(),
            }])?;
        }
        Ok(())
    }

    fn converge_dsh_binding(
        &self,
        runtime: &Arc<Server>,
        saved: &RuntimeBinding,
        mode: DshConvergence,
    ) -> anyhow::Result<()> {
        let context_identity = RuntimeIdentity {
            agent_id: saved.agent_id.clone(),
            runtime_id: saved.runtime_id.clone(),
            appserver_id: saved.app_scope_id.clone(),
            binding_id: saved.binding_id.clone(),
            endpoint_generation: saved.endpoint_generation,
            session_id: saved.session_id.clone(),
            native_thread_id: saved.native_thread_id.clone(),
        };
        let context = ProjectContext::for_registered_route(&runtime.root, &context_identity)?;
        let mut ident = {
            let state = runtime.state.lock().unwrap();
            let current = state
                .global
                .lookup_binding_for(&saved.route_scope(), &saved.binding_id);
            if current != Some(saved) {
                anyhow::bail!("RECOVERY_RECONCILE_REQUIRED: binding changed");
            }
            self.recovered_binding_identity(&context, saved, &state)
                .ok_or_else(|| anyhow::anyhow!("IDENTITY_WORKER_MISSING"))?
        };
        let transport = ident
            .transport
            .clone()
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_TRANSPORT_MISSING"))?;
        if transport.kind != TransportKind::Dsh {
            return Ok(());
        }
        let enroll = |identity: &identity::Identity, selected: &SelectedTransport| {
            let binding = identity
                .runtime
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("IDENTITY_RUNTIME_MISSING"))?;
            self.enroll_dsh_receive_loop(
                selected,
                &identity.worker_id,
                &identity.token,
                context.project_scope.as_str(),
                crate::client::adapters::dsh::DshRuntimeContext {
                    agent_id: binding.agent_id.as_str(),
                    runtime_id: binding.runtime_id.as_str(),
                    appserver_id: binding.appserver_id.as_str(),
                    binding_id: binding.binding_id.as_str(),
                    endpoint_generation: binding.endpoint_generation,
                    session_id: binding.session_id.as_ref().map(|id| id.as_str()),
                    native_thread_id: binding.native_thread_id.as_ref().map(|id| id.as_str()),
                },
            )
        };
        if matches!(mode, DshConvergence::ReceiveLoop) {
            match enroll(&ident, &transport) {
                Ok(()) => return Ok(()),
                Err(error) => match dsh_control_error(&error) {
                    Some(control) if control.is_definitely_absent() => {
                        return self.retire_absent_dsh_binding(runtime, saved);
                    }
                    Some(crate::client::adapters::dsh::ControlError::Rejected { code, .. })
                        if code == "unknown-runtime" => {}
                    _ => return Err(error),
                },
            }
        }
        // Discovery is scoped to the committed endpoint/session, and performs
        // no identity mutation. A down socket or ambiguous host stays pending.
        let endpoint = transport
            .endpoint
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("DSH_ENDPOINT_MISSING"))?;
        let session = saved
            .session_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("DSH_SESSION_MISSING"))?;
        let agent = saved
            .native_thread_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("DSH_AGENT_MISSING"))?;
        if matches!(mode, DshConvergence::Endpoint) {
            let namespace = transport
                .namespace
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("DSH_RUNTIME_MISSING"))?;
            match crate::client::adapters::dsh::facts(endpoint, namespace, agent.as_str()) {
                Ok(current)
                    if current.runtime_id == namespace
                        && current.agent_id == agent.as_str()
                        && current.session_id == session.as_str()
                        && std::fs::canonicalize(&current.cwd)? == runtime.root =>
                {
                    return Ok(())
                }
                Ok(_) => anyhow::bail!(
                    "DSH_ENDPOINT_REJECTED: current host facts do not match the binding"
                ),
                Err(crate::client::adapters::dsh::ControlError::Rejected { code, .. })
                    if code == "unknown-runtime" => {}
                Err(error) => return Err(anyhow::Error::new(error)),
            }
        }
        let resolved =
            match crate::client::adapters::dsh::resolve_agent(
                endpoint,
                agent.as_str(),
                session.as_str(),
                &runtime.root,
            ) {
                Ok(resolved) => resolved,
                Err(error)
                    if matches!(mode, DshConvergence::ReceiveLoop)
                        && error.is_definitely_absent() =>
                {
                    return self.retire_absent_dsh_binding(runtime, saved);
                }
                Err(error) => return Err(anyhow::Error::new(error)),
            };
        let mut next = transport.clone();
        next.namespace = Some(resolved.runtime_id);
        if next == transport {
            // Keep the actual enrollment failure, including a rejected receipt.
            return if matches!(mode, DshConvergence::ReceiveLoop) {
                enroll(&ident, &transport)
            } else {
                Ok(())
            };
        }
        {
            let _guard = self.identity_gate.lock().unwrap();
            let state = runtime.state.lock().unwrap();
            if state
                .global
                .lookup_binding_for(&saved.route_scope(), &saved.binding_id)
                != Some(saved)
                || !state.workers.get(&ident.worker_id).is_some_and(|worker| {
                    worker.token == ident.token && worker.transport.as_ref() == Some(&transport)
                })
            {
                anyhow::bail!("RECOVERY_RECONCILE_REQUIRED: binding changed during host discovery");
            }
            drop(state);
            self.refresh_session_transport(
                runtime,
                saved,
                next.clone(),
                &mut ident,
                &Scope {
                    root: runtime.root.clone(),
                },
            )?;
        }
        enroll(&ident, &next)
    }

    /// Run before context takes the identity gate. This uses the same owner as
    /// background convergence and never broadens an explicit endpoint report.
    fn reconcile_current_dsh_session(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
    ) -> anyhow::Result<()> {
        if facts.dsh_session_id.is_none() && facts.dsh.is_none() {
            return Ok(());
        }
        let mut anchors = facts.clone();
        anchors.dsh = None;
        let Some(ident) = self.current_session_identity(context, &anchors)? else {
            return Ok(());
        };
        let Some(transport) = ident
            .transport
            .as_ref()
            .filter(|t| t.kind == TransportKind::Dsh)
        else {
            return Ok(());
        };
        if facts.dsh.as_ref().is_some_and(|candidate| {
            transport.endpoint.as_deref() != Some(candidate.endpoint.as_str())
                || transport.session_id.as_deref() != Some(candidate.session_id.as_str())
                || transport.thread_id.as_deref() != Some(candidate.agent_id.as_str())
        }) {
            return Ok(());
        }
        let runtime = self.select_runtime(context).map_err(anyhow::Error::msg)?;
        let binding = ident
            .runtime
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_RUNTIME_MISSING"))?;
        let saved = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(
                &RouteScope {
                    app_scope_id: context.app_scope_id.clone(),
                    project_scope_id: context.project_scope.clone(),
                },
                &binding.binding_id,
            )
            .cloned();
        if let Some(saved) = saved {
            self.converge_dsh_binding(&runtime, &saved, DshConvergence::Endpoint)?;
        }
        Ok(())
    }
}
