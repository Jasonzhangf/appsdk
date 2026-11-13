fn verify_local_rex_socket(server: &str) -> anyhow::Result<()> {
    let path = server
        .strip_prefix("unix://")
        .ok_or_else(|| anyhow::anyhow!("REX_EXECUTION_UNVERIFIED: Rex endpoint is not a local unix socket"))?;
    use std::os::unix::ffi::OsStrExt;
    let decoded_path = percent_encoding::percent_decode_str(path).collect::<Vec<_>>();
    let path = Path::new(std::ffi::OsStr::from_bytes(&decoded_path));
    if !path.is_absolute() {
        anyhow::bail!("REX_EXECUTION_UNVERIFIED: Rex unix socket path is not absolute");
    }
    use std::os::unix::fs::FileTypeExt;
    let metadata = std::fs::metadata(path)
        .map_err(|error| anyhow::anyhow!("REX_EXECUTION_UNVERIFIED: Rex socket: {error}"))?;
    if !metadata.file_type().is_socket() {
        anyhow::bail!("REX_EXECUTION_UNVERIFIED: Rex endpoint is not a unix socket");
    }
    Ok(())
}

fn rex_execution_start_clock(
    start_id: &str,
) -> Option<crate::server::binding_slot::RexExecutionFence> {
    crate::server::binding_slot::RexExecutionFence::from_start_id(start_id)
}

fn classify_rex_execution(
    baseline: Option<&RexRecoveryBaseline>,
    current_session: Option<&str>,
    execution: crate::server::binding_slot::VerifiedRexExecutionAssociation,
) -> RexRecoveryAdmission {
    let Some(baseline) = baseline else {
        return RexRecoveryAdmission::Verified(execution);
    };
    let session_same = baseline
        .current
        .binding
        .session_id
        .as_ref()
        .map(identity::SessionId::as_str)
        == current_session;
    let Some(previous) = baseline.current.rex_execution.as_ref() else {
        if !session_same {
            if let Some(fence) = baseline.current.rex_execution_fence.as_ref() {
                let candidate = rex_execution_start_clock(&execution.process_start_id);
                return match candidate.as_ref().and_then(|candidate| candidate.compare(fence)) {
                    Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Less) => {
                        RexRecoveryAdmission::DisplacedExecution(execution)
                    }
                    None => RexRecoveryAdmission::Missing(
                        "REX_EXECUTION_ASSOCIATION_REQUIRED: precise execution start order is unavailable"
                            .into(),
                    ),
                    Some(std::cmp::Ordering::Greater) => RexRecoveryAdmission::Missing(
                        "REX_EXECUTION_ASSOCIATION_REQUIRED: current binding has no verified Rex execution association"
                            .into(),
                    ),
                };
            }
        }
        return if session_same {
            RexRecoveryAdmission::Verified(execution)
        } else {
            RexRecoveryAdmission::Missing(
                "REX_EXECUTION_ASSOCIATION_REQUIRED: current binding has no verified Rex execution association"
                    .into(),
            )
        };
    };
    if previous == &execution {
        return if session_same {
            RexRecoveryAdmission::Verified(execution)
        } else {
            RexRecoveryAdmission::Missing(
                "REX_EXECUTION_ASSOCIATION_REQUIRED: the same Rex execution cannot prove a new agent session"
                    .into(),
            )
        };
    }
    if previous.owner_pid == execution.owner_pid
        && previous.process_start_id == execution.process_start_id
    {
        return RexRecoveryAdmission::Missing(
            "REX_EXECUTION_ASSOCIATION_REQUIRED: the same Rex execution cannot prove a new agent session"
                .into(),
        );
    }
    let previous_start = rex_execution_start_clock(&previous.process_start_id);
    let next_start = rex_execution_start_clock(&execution.process_start_id);
    if let Some(fence) = baseline.current.rex_execution_fence.as_ref() {
        return match next_start.as_ref().and_then(|start| start.compare(fence)) {
            Some(std::cmp::Ordering::Equal | std::cmp::Ordering::Less) => {
                RexRecoveryAdmission::DisplacedExecution(execution)
            }
            None => RexRecoveryAdmission::Missing(
                "REX_EXECUTION_ASSOCIATION_REQUIRED: precise execution start order is unavailable"
                    .into(),
            ),
            Some(std::cmp::Ordering::Greater) => {
                return match previous_start.as_ref().and_then(|previous| {
                    next_start.as_ref().and_then(|next| next.compare(previous))
                }) {
                    Some(std::cmp::Ordering::Greater) => {
                        RexRecoveryAdmission::Verified(execution)
                    }
                    Some(std::cmp::Ordering::Less) => RexRecoveryAdmission::Missing(
                        "REX_EXECUTION_ASSOCIATION_REQUIRED: caller is older than the current Rex execution"
                            .into(),
                    ),
                    Some(std::cmp::Ordering::Equal) => RexRecoveryAdmission::Missing(
                        "REX_EXECUTION_ASSOCIATION_REQUIRED: equal execution start ticks cannot establish a new execution"
                            .into(),
                    ),
                    _ => RexRecoveryAdmission::Missing(
                        "REX_EXECUTION_ASSOCIATION_REQUIRED: precise execution start order is unavailable"
                            .into(),
                    ),
                };
            }
        };
    }
    match next_start.as_ref().and_then(|next| {
        previous_start
            .as_ref()
            .and_then(|previous| next.compare(previous))
    }) {
        Some(std::cmp::Ordering::Greater) => {
            RexRecoveryAdmission::Verified(execution)
        }
        Some(std::cmp::Ordering::Less) => {
            RexRecoveryAdmission::Missing(
                "REX_EXECUTION_ASSOCIATION_REQUIRED: caller is older than the current Rex execution"
                    .into(),
            )
        }
        _ => RexRecoveryAdmission::Missing(
            "REX_EXECUTION_ASSOCIATION_REQUIRED: precise execution start order is unavailable"
                .into(),
        ),
    }
}

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
        if let Some(_candidate) = facts.rex.as_ref() {
            facts.tmux = None;
            facts.tmux_anchor = None;
            facts.tmux_session_name = None;
            facts.pane_hint = None;
            return Ok((facts, None));
        }
        if facts.dsh.is_some() || facts.dsh_session_id.is_some() {
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

    /// Observe one explicit Rex invocation without holding State or the
    /// register gate. The map baseline is read first; all later admission uses
    /// that generation rather than a post-observation replacement.
    fn observe_rex_recovery(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
        invocation_cwd: Option<&str>,
    ) -> anyhow::Result<RexRecoveryObservation> {
        let candidate = facts
            .rex
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("REX_EXECUTION_OBSERVATION_REQUIRED"))?
            .clone();
        let control = self
            .binding_control_store()
            .load()
            .map_err(anyhow::Error::msg)?;
        let baseline = control
            .lookup(
                &context.project_scope,
                &context.app_scope_id,
                facts,
                invocation_cwd,
            )
            .map_err(anyhow::Error::msg)?
            .map(|slot| RexRecoveryBaseline {
                key: slot.identity_key.clone(),
                generation: slot.generation(),
                current: slot
                    .active_binding()
                    .expect("lookup returns active current")
                    .clone(),
            });
        let candidates = TransportCandidates {
            rex: Some(candidate.clone()),
            ..Default::default()
        };
        let transport = validate_transport_candidates_for_session(
            &self.host,
            &candidates,
            &context.canonical_root,
            facts.session_id.as_deref(),
        )
        .map_err(anyhow::Error::msg)?;
        let endpoint = transport
            .rex_endpoint
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("REX_ENDPOINT_REJECTED: selected transport lost its endpoint"))?;
        verify_local_rex_socket(&endpoint.server)?;
        let handle = tokio::runtime::Handle::try_current().map_err(|_| {
            anyhow::anyhow!("REX_EXECUTION_UNVERIFIED: daemon runtime is unavailable")
        })?;
        let observed = handle
            .block_on(crate::client::adapters::rex::observe_current(endpoint))
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let owner = match observed.current_foreground_codex_owner() {
            Ok(owner) => owner,
            Err(reason) => {
                return Ok(RexRecoveryObservation {
                    baseline,
                    transport,
                    admission: RexRecoveryAdmission::Missing(reason),
                });
            }
        };
        let owner_pid = u32::try_from(owner.pid).map_err(|_| {
            anyhow::anyhow!("REX_EXECUTION_UNVERIFIED: foreground PID is outside the process ID range")
        })?;
        let start_before = crate::host_runtime::exact_process_start_id(owner_pid)
            .map_err(anyhow::Error::msg)?;
        let start_after = crate::host_runtime::exact_process_start_id(owner_pid)
            .map_err(anyhow::Error::msg)?;
        if start_before.is_none() || start_before != start_after {
            return Ok(RexRecoveryObservation {
                baseline,
                transport,
                admission: RexRecoveryAdmission::Missing(
                    "REX_EXECUTION_UNVERIFIED: owner start identity changed during observation"
                        .into(),
                ),
            });
        }
        let start_id = start_before.expect("matched start identity");
        let Some(caller_pid) = facts.caller_pid else {
            return Ok(RexRecoveryObservation {
                baseline,
                transport,
                admission: RexRecoveryAdmission::Missing(
                    "REX_EXECUTION_UNVERIFIED: current invocation has no caller PID".into(),
                ),
            });
        };
        let nearest = crate::host_runtime::caller_nearest_agent(caller_pid)
            .map_err(anyhow::Error::msg)?;
        let Some((nearest_pid, nearest_kind)) = nearest else {
            return Ok(RexRecoveryObservation {
                baseline,
                transport,
                admission: RexRecoveryAdmission::CallerMismatch(
                    "REX_EXECUTION_CALLER_MISMATCH: caller has no Codex ancestor".into(),
                ),
            });
        };
        if nearest_pid != owner_pid || nearest_kind != crate::host_runtime::AgentKind::Codex {
            return Ok(RexRecoveryObservation {
                baseline,
                transport,
                admission: RexRecoveryAdmission::CallerMismatch(format!(
                    "REX_EXECUTION_CALLER_MISMATCH: caller Codex owner pid={nearest_pid} differs from Rex foreground pid={owner_pid}"
                )),
            });
        }
        let execution =
            crate::server::binding_slot::VerifiedRexExecutionAssociation {
                owner_pid,
                owner_name: owner.name,
                process_start_id: start_id,
            };
        let admission = classify_rex_execution(
            baseline.as_ref(),
            facts.session_id.as_deref(),
            execution,
        );
        Ok(RexRecoveryObservation {
            baseline,
            transport,
            admission,
        })
    }

    fn current_session_identity(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
    ) -> anyhow::Result<Option<identity::Identity>> {
        self.current_session_identity_at_cwd(context, facts, None, false)
    }

    fn current_session_identity_at_cwd(
        &self,
        context: &ProjectContext,
        facts: &IdentityFacts,
        invocation_cwd: Option<&str>,
        allow_explicit_rex_rebind: bool,
    ) -> anyhow::Result<Option<identity::Identity>> {
        let control = self
            .binding_control_store()
            .load()
            .map_err(anyhow::Error::msg)?;
        let lookup = if allow_explicit_rex_rebind {
            control.lookup_explicit_rex_rebind(
                &context.project_scope,
                &context.app_scope_id,
                facts,
                invocation_cwd,
            )
        } else {
            control.lookup(
                &context.project_scope,
                &context.app_scope_id,
                facts,
                invocation_cwd,
            )
        };
        let Some(slot) = lookup
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

        self.refresh_session_transport(
            runtime,
            saved,
            transport,
            ident,
            scope,
            None,
            None,
        )
    }

    fn rex_execution_for_refresh(
        &self,
        recovery: Option<&RexRecoveryObservation>,
        saved: &RuntimeBinding,
        candidate: &crate::proto::RexCandidate,
        session: Option<&str>,
        allow_explicit_rex_rebind: bool,
    ) -> anyhow::Result<(
        Option<crate::server::binding_slot::VerifiedRexExecutionAssociation>,
        Option<RexRecoveryBaseline>,
    )> {
        if allow_explicit_rex_rebind {
            return Ok((None, None));
        }
        let Some(recovery) = recovery else {
            if saved.session_id.as_ref().map(identity::SessionId::as_str) != session {
                anyhow::bail!(
                    "REX_EXECUTION_ASSOCIATION_REQUIRED: automatic Rex session change has no current execution observation"
                );
            }
            return Ok((None, None));
        };
        if let Some(baseline) = recovery.baseline.as_ref() {
            if baseline.current.binding != *saved {
                anyhow::bail!(
                    "IDENTITY_MAP_BINDING_CHANGED: Rex recovery baseline no longer matches the published current"
                );
            }
        }
        match &recovery.admission {
            RexRecoveryAdmission::Verified(execution) => {
                Ok((Some(execution.clone()), recovery.baseline.clone()))
            }
            RexRecoveryAdmission::CallerMismatch(reason) => {
                anyhow::bail!("{reason}")
            }
            RexRecoveryAdmission::DisplacedExecution(_) => {
                anyhow::bail!("REX_EXECUTION_DISPLACED: human replacement excludes this execution from the target name")
            }
            RexRecoveryAdmission::Missing(reason) => {
                if recovery.baseline.is_none()
                    || saved.session_id.as_ref().map(identity::SessionId::as_str) == session
                {
                    Ok((None, recovery.baseline.clone()))
                } else {
                    anyhow::bail!("{reason}")
                }
            }
        }
    }

    fn refresh_session_transport(
        &self,
        runtime: &Server,
        saved: &RuntimeBinding,
        transport: SelectedTransport,
        ident: &mut identity::Identity,
        scope: &Scope,
        rex_execution: Option<&crate::server::binding_slot::VerifiedRexExecutionAssociation>,
        expected_baseline: Option<&RexRecoveryBaseline>,
    ) -> anyhow::Result<RuntimeBinding> {
        let _register = self.register_gate.lock().unwrap();
        if let (Some(expected), Some(execution)) = (expected_baseline, rex_execution) {
            if expected.current.rex_execution.as_ref() == Some(execution) {
                // The association is already part of the exact baseline.
            } else {
                let current_start = crate::host_runtime::exact_process_start_id(execution.owner_pid)
                    .map_err(|error| anyhow::anyhow!(error))?;
                if current_start.as_deref() != Some(execution.process_start_id.as_str()) {
                    anyhow::bail!(
                        "REX_EXECUTION_UNVERIFIED: current execution changed before commit"
                    );
                }
            }
        }
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
        if same_placement && published {
            let association_unchanged = rex_execution
                .map(|execution| {
                    expected_baseline
                        .and_then(|baseline| baseline.current.rex_execution.as_ref())
                        == Some(execution)
                })
                .unwrap_or_else(|| {
                    expected_baseline.map_or(true, |baseline| {
                        baseline.current.rex_execution.is_none()
                    })
                });
            if association_unchanged
                && worker.transport.as_ref() == Some(&transport)
                && ident.transport.as_ref() == Some(&transport)
            {
                return Ok(saved.clone());
            }
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
        let (key, expected_generation) = if let Some(expected) = expected_baseline {
            let control = self.binding_control_store().load().map_err(anyhow::Error::msg)?;
            if control.slot(&expected.key).map(|slot| (slot.generation(), slot.active_binding()))
                != Some((expected.generation, Some(&expected.current)))
            {
                anyhow::bail!(
                    "IDENTITY_MAP_BINDING_CHANGED: Rex recovery baseline changed before commit"
                );
            }
            (expected.key.clone(), expected.generation)
        } else {
            self.mapped_key_and_generation_for_worker(&control_context, &ident.worker_id, saved)
                .map_err(anyhow::Error::msg)?
        };
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
        if transport.kind == TransportKind::Rex && rex_execution.is_none()
            && expected_baseline.is_none() && next.session_id != saved.session_id {
            if let Some(target) = control.slot(&key).and_then(|slot| slot.active_binding()) {
                value.replace_rex_holder(target).map_err(anyhow::Error::msg)?;
            }
        }
        if let Some(execution) = rex_execution {
            value.rex_execution = Some(execution.clone());
        } else if let Some(baseline) = expected_baseline {
            if next.binding_id == baseline.current.binding.binding_id
                && next.session_id == baseline.current.binding.session_id
                && transport.rex_endpoint == baseline.current.transport.rex_endpoint
            {
                value.rex_execution = baseline.current.rex_execution.clone();
            }
        }
        let rename = crate::server::identity_context::rename_prepared_anchor(
            &value,
            &value.invocation_cwd, &key.name,
            current_master_worker_id(&state, Some(&saved.route_scope())).as_deref() == Some(ident.worker_id.as_str()),
        ).map_err(anyhow::Error::msg)?;
        let committed = match self.commit_control_change_locked(
            crate::server::binding_store::ControlChange::CommitWithEffect {
                key: key.clone(),
                value,
                expected_generation: Some(expected_generation),
                effect: effect.clone(),
            },
        ) {
            Ok(committed) => committed,
            Err(error) => {
                let error = match rename.rollback() {
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

    pub(crate) fn refresh_session_transport_for_notification(
        &self,
        runtime: &Server,
        saved: &RuntimeBinding,
        transport: SelectedTransport,
        ident: &mut identity::Identity,
        scope: &Scope,
    ) -> anyhow::Result<RuntimeBinding> {
        if transport.kind != TransportKind::Rex {
            anyhow::bail!("REX_CURRENT_BINDING_CHANGED: notification refresh requires Rex transport");
        }
        self.refresh_session_transport(runtime, saved, transport, ident, scope, None, None)
    }

    fn reconcile_identity_context(
        &self,
        context: ProjectContext,
        mut facts: IdentityFacts,
        outer_binding: Option<RegisterOuterBinding>,
        allow_persisted_restore: bool,
        allow_explicit_rex_rebind: bool,
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
        let rex_recovery = outer_binding
            .as_ref()
            .and_then(|binding| binding.rex_recovery().cloned());
        // The caller has already performed the single live observation and
        // cleared caller_pid/pane_hint. Do not observe a second time. The pane
        // anchor is derived only from that invocation's live observation by
        // `resolve_context_name`; persisted or caller-supplied facts.tmux is
        // never used as a live pane here.
        validate_facts(&facts)?;
        let scope = Scope { root };
        let displaced = rex_recovery.as_ref().is_some_and(|recovery|
            matches!(recovery.admission, RexRecoveryAdmission::DisplacedExecution(_)));
        let session_identity = if displaced {
            // The endpoint still belongs to the human-selected current. A
            // verified displaced caller registers independently of that owner.
            None
        } else { self.current_session_identity_at_cwd(
            &context,
            &facts,
            invocation_cwd,
            allow_explicit_rex_rebind,
        )? };
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
                                None, None,
                            )?);
                        } else if let Some(candidate) = facts.rex.as_ref() {
                            let (execution, baseline) = self.rex_execution_for_refresh(
                                rex_recovery.as_ref(),
                                saved,
                                candidate,
                                facts.session_id.as_deref(),
                                allow_explicit_rex_rebind,
                            )?;
                            let candidates = TransportCandidates { rex: Some(candidate.clone()), ..Default::default() };
                            transport = rex_recovery
                                .as_ref()
                                .map(|recovery| recovery.transport.clone())
                                .unwrap_or(
                                    validate_transport_candidates_for_session(
                                        &runtime,
                                        &candidates,
                                        &context.canonical_root,
                                        facts.session_id.as_deref(),
                                    )
                                    .map_err(anyhow::Error::msg)?,
                                );
                            current = Some(self.refresh_session_transport(
                                &runtime,
                                saved,
                                transport,
                                &mut ident,
                                &scope,
                                execution.as_ref(),
                                baseline.as_ref(),
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
                                None, None,
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
                                None, None,
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
                                None, None,
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
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::Arc;

    #[test]
    fn rex_execution_classifier_uses_only_comparable_clock_domains() {
        use crate::server::binding_slot::{
            BindingValue, IdentityBindingKey, RexExecutionFence, VerifiedRexExecutionAssociation,
        };
        use std::cmp::Ordering;

        let unix_earlier = rex_execution_start_clock("9:900000").unwrap();
        let unix_later = rex_execution_start_clock("10:3").unwrap();
        assert_eq!(
            unix_earlier.partial_cmp(&unix_later),
            Some(Ordering::Less)
        );
        assert_eq!(rex_execution_start_clock("unavailable"), None);
        assert_eq!(rex_execution_start_clock("10:invalid"), None);

        let (_, root) = test_server();
        let context = recovery_context(&root);
        let key = IdentityBindingKey::new(
            context.project_scope.clone(),
            context.app_scope_id.clone(),
            "Alice",
        );
        let binding = RuntimeBinding::new_with_session(
            context.project_scope.clone(),
            context.app_scope_id.clone(),
            identity::AgentId::new("actor").unwrap(),
            identity::RuntimeId::new("runtime").unwrap(),
            identity::BindingId::new("binding").unwrap(),
            Some(identity::SessionId::new("s0").unwrap()),
            None,
        )
        .unwrap();
        let execution = |pid, start: &str| VerifiedRexExecutionAssociation {
            owner_pid: pid,
            owner_name: "codex".into(),
            process_start_id: start.into(),
        };
        let boot = "b879bb0a-cf9a-47d1-b889-f63aa872d851";
        let other_boot = "11111111-1111-1111-1111-111111111111";
        let mut current = BindingValue::new(binding, rex_transport(&root, "s0"));
        current.rex_execution = Some(execution(10, &format!("linux:{boot}:100:1000")));
        current.rex_execution_fence = None;
        let baseline = RexRecoveryBaseline {
            key,
            generation: 1,
            current,
        };

        assert!(matches!(
            classify_rex_execution(
                Some(&baseline),
                Some("s1"),
                execution(20, &format!("linux:{boot}:100:1001"))
            ),
            RexRecoveryAdmission::Verified(_)
        ));
        assert!(matches!(
            classify_rex_execution(
                Some(&baseline),
                Some("s1"),
                execution(20, &format!("linux:{boot}:100:1000"))
            ),
            RexRecoveryAdmission::Missing(_)
        ));
        assert!(matches!(
            classify_rex_execution(
                Some(&baseline),
                Some("s1"),
                execution(20, &format!("linux:{other_boot}:100:9999"))
            ),
            RexRecoveryAdmission::Missing(_)
        ));
        assert!(matches!(
            classify_rex_execution(Some(&baseline), Some("s1"), execution(20, "12:7")),
            RexRecoveryAdmission::Missing(_)
        ));

        let mut fenced = baseline.clone();
        fenced.current.rex_execution_fence = Some(
            RexExecutionFence::from_start_id(&format!("linux:{boot}:100:900")).unwrap(),
        );
        assert!(matches!(
            classify_rex_execution(
                Some(&fenced),
                Some("s1"),
                execution(20, &format!("linux:{boot}:100:800"))
            ),
            RexRecoveryAdmission::DisplacedExecution(_)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rex_human_fence_admission_preserves_current_and_allows_future_execution() {
        use crate::server::binding_slot::{BindingValue, IdentityBindingKey, RexExecutionFence, VerifiedRexExecutionAssociation};
        let (_, root) = test_server();
        let context = recovery_context(&root);
        let key = IdentityBindingKey::new(context.project_scope.clone(), context.app_scope_id.clone(), "Alice");
        let binding = RuntimeBinding::new_with_session(
            context.project_scope.clone(), context.app_scope_id.clone(),
            identity::AgentId::new("actor").unwrap(), identity::RuntimeId::new("runtime").unwrap(),
            identity::BindingId::new("binding").unwrap(), Some(identity::SessionId::new("s0").unwrap()), None,
        ).unwrap();
        let execution = |pid, start: &str| VerifiedRexExecutionAssociation {
            owner_pid: pid, owner_name: "codex".into(), process_start_id: start.into(),
        };
        let mut current = BindingValue::new(binding, rex_transport(&root, "s0"));
        current.rex_execution = Some(execution(10, "100:123456"));
        current.rex_execution_fence = RexExecutionFence::from_start_id("200:654321");
        let mut baseline = RexRecoveryBaseline { key, generation: 4, current };
        assert!(matches!(classify_rex_execution(Some(&baseline), Some("s0"), execution(10, "100:123456")), RexRecoveryAdmission::Verified(_)));
        assert!(matches!(classify_rex_execution(Some(&baseline), Some("s1"), execution(20, "200:654321")), RexRecoveryAdmission::DisplacedExecution(_)));
        assert!(matches!(classify_rex_execution(Some(&baseline), Some("s1"), execution(20, "150:1")), RexRecoveryAdmission::DisplacedExecution(_)));
        assert!(matches!(classify_rex_execution(Some(&baseline), Some("s3"), execution(30, "200:654322")), RexRecoveryAdmission::Verified(_)));
        assert!(matches!(classify_rex_execution(Some(&baseline), Some("changed-session"), execution(10, "100:123456")), RexRecoveryAdmission::Missing(_)));
        baseline.current.rex_execution = None;
        assert!(matches!(classify_rex_execution(Some(&baseline), Some("s0"), execution(10, "100:123456")), RexRecoveryAdmission::Verified(_)), "restored current may establish association below the fence");
        assert!(matches!(classify_rex_execution(Some(&baseline), Some("s1"), execution(20, "150:1")), RexRecoveryAdmission::DisplacedExecution(_)));
        assert!(matches!(classify_rex_execution(Some(&baseline), Some("s3"), execution(30, "300:1")), RexRecoveryAdmission::Missing(_)), "future execution still needs a comparable current association");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_rex_socket_verification_decodes_uri_path() {
        let (_, root) = test_server();
        let socket = root.join("rex socket.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = format!("unix://{}", socket.display()).replace(' ', "%20");
        assert!(verify_local_rex_socket(&server).is_ok(),
            "the official Rex endpoint URI must resolve its encoded filesystem path");
        let regular = root.join("regular file");
        std::fs::write(&regular, "not a socket").unwrap();
        let regular_uri = format!("unix://{}", regular.display()).replace(' ', "%20");
        assert!(verify_local_rex_socket(&regular_uri).unwrap_err().to_string().contains("not a unix socket"));
        drop(listener);
        std::fs::remove_dir_all(root).unwrap();
    }

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

    struct CodexCallerFixture {
        owner: std::process::Child,
        owner_pid: u32,
        caller_pid: u32,
        caller_start_id: String,
    }

    impl CodexCallerFixture {
        fn stop(self) {
            drop(self);
        }
    }

    impl Drop for CodexCallerFixture {
        fn drop(&mut self) {
            stop_exact_process(self.caller_pid, &self.caller_start_id);
            self.owner.kill().ok();
            self.owner.wait().ok();
        }
    }

    fn spawn_codex_caller(
        root: &std::path::Path,
        label: &str,
    ) -> CodexCallerFixture {
        let codex = root.join("codex");
        match std::fs::symlink_metadata(&codex) {
            Ok(metadata) if metadata.file_type().is_symlink() => {}
            Ok(_) => panic!("private fixture codex path exists and is not a symlink"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::os::unix::fs::symlink("/bin/sh", &codex)
                    .expect("create private codex test symlink");
            }
            Err(error) => panic!("inspect private codex test symlink: {error}"),
        }
        let caller_file = root.join(format!("{label}.caller-pid"));
        std::fs::remove_file(&caller_file).ok();
        let script = format!(
            "sleep 30 & echo $! > {}; wait",
            caller_file.to_string_lossy()
        );
        let owner = Command::new(&codex)
            .arg("-c")
            .arg(script)
            .spawn()
            .expect("spawn codex owner fixture");
        let owner_pid = owner.id();
        for _ in 0..100 {
            if let Ok(text) = std::fs::read_to_string(&caller_file) {
                if let Ok(caller_pid) = text.trim().parse::<u32>() {
                    if caller_pid > 1 {
                        let caller_start_id =
                            crate::host_runtime::exact_process_start_id(caller_pid)
                                .unwrap()
                                .expect("codex caller fixture start identity");
                        return CodexCallerFixture {
                            owner,
                            owner_pid,
                            caller_pid,
                            caller_start_id,
                        };
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("codex caller fixture did not publish its child PID");
    }

    fn stop_exact_process(pid: u32, expected_start_id: &str) {
        if pid <= 1 {
            return;
        }
        let current = crate::host_runtime::exact_process_start_id(pid).unwrap();
        if current.as_deref() != Some(expected_start_id) {
            return;
        }
        let result = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        assert_eq!(result, 0, "stop exact fixture process pid={pid}");
    }

    fn rex_transport(root: &std::path::Path, session: &str) -> crate::proto::SelectedTransport {
        crate::proto::SelectedTransport {
            kind: crate::proto::TransportKind::Rex,
            endpoint: None,
            namespace: None,
            session_id: Some(session.into()),
            thread_id: None,
            tmux_endpoint: None,
            rex_endpoint: Some(crate::proto::RexEndpoint {
                server: format!("unix:///tmp/collab-human-fence-{}.sock", std::process::id()),
                session_id: "session:11111111-1111-1111-1111-111111111111".into(),
                block_id: "block:22222222-2222-2222-2222-222222222222".into(),
                host_id: Some("host-fixture".into()),
                instance_id: "instance-fixture".into(),
                cwd: root.display().to_string(),
            }),
            capabilities: vec!["send_message_to_block".into()],
            self_check: "rex execution association test".into(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rex_execution_association_continues_newer_execution_and_rejects_older_execution() {
        use crate::client::adapters::rex::{
            set_test_cli, set_test_timeout, TEST_FIXTURE_LOCK,
        };

        let _guard = TEST_FIXTURE_LOCK.lock().await;
        let (server, root) = test_server();
        let fixture_dir = root.join("rex-fixture");
        std::fs::create_dir_all(&fixture_dir).unwrap();
        let fixture = fixture_dir.join("rex-cli");
        std::fs::write(&fixture, include_str!("../../adapters/rex/fixture_rex.py")).unwrap();
        let mut permissions = std::fs::metadata(&fixture).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&fixture, permissions).unwrap();
        set_test_cli(Some(fixture));
        set_test_timeout(Some(std::time::Duration::from_secs(5)));
        std::env::set_var("REXPRIMITIVE_LOG_DIR", &fixture_dir);
        let socket_path = format!("/tmp/collab-human-fence-{}.sock", std::process::id());
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();

        let transport = rex_transport(&root, "session-s0");
        let seeded = crate::server::peer_tests::seed_committed_owner(
            &server,
            "rex-execution-owner",
            "token-rex-execution-owner",
            &transport,
            &root.display().to_string(),
            Some(AppServerId::new(crate::identity::CLI_APP_SERVER_ID).unwrap()),
        );
        assert!(seeded.ok, "{seeded:?}");
        let host_paths = server.host_paths.clone();
        let control = crate::server::binding_store::BindingControlStore::new(
            host_paths.state_root(),
        )
        .load()
        .unwrap();
        let current = control
            .active_values()
            .find(|value| value.binding.agent_id.as_str() == "rex-execution-owner")
            .cloned()
            .expect("seeded Rex owner current");
        let mut credential = crate::identity::Identity {
            worker_id: "rex-execution-owner".into(),
            token: "token-rex-execution-owner".into(),
            project_scope: None,
            runtime: None,
            transport: None,
        };
        crate::identity::persist_current_binding_at(
            &host_paths,
            &Scope { root: root.clone() },
            &mut credential,
            crate::identity::RuntimeIdentity {
                agent_id: current.binding.agent_id.clone(),
                runtime_id: current.binding.runtime_id.clone(),
                appserver_id: current.binding.app_scope_id.clone(),
                binding_id: current.binding.binding_id.clone(),
                session_id: current.binding.session_id.clone(),
                native_thread_id: current.binding.native_thread_id.clone(),
            },
            current.transport.clone(),
        )
        .unwrap();
        let manager = Arc::new(ProjectRuntimeManager::new(Arc::new(server), &host_paths).unwrap());
        let context = recovery_context(&root);

        let owner0 = spawn_codex_caller(&root, "execution-owner-0");
        std::env::set_var("COLLAB_TEST_REX_OWNER_PID", owner0.owner_pid.to_string());
        std::env::set_var("COLLAB_TEST_REX_OWNER_NAME", "codex");
        let request0 = IdentityContextRequest {
            operation_id: "rex-execution-s0".into(),
            action: "context".into(),
            invocation: "automatic".into(),
            facts: IdentityFacts {
                rex: Some(crate::proto::RexCandidate {
                    server: format!("unix://{socket_path}"),
                    session_id: "session:11111111-1111-1111-1111-111111111111".into(),
                    block_id: "block:22222222-2222-2222-2222-222222222222".into(),
                    cwd: root.display().to_string(),
                }),
                session_id: Some("session-s0".into()),
                caller_pid: Some(owner0.caller_pid),
                ..IdentityFacts::default()
            },
            invocation_cwd: Some(root.display().to_string()),
            approval: None,
            grant_approval: None,
            query: false,
        };
        let (_, first) = tokio::task::spawn_blocking({
            let manager = manager.clone();
            let context = context.clone();
            let request = request0.clone();
            move || manager.mapped_context(context, request)
        })
        .await
        .unwrap();
        assert!(first.ok, "first verified Rex context must succeed: {first:?}");
        let map_after_first: serde_json::Value = serde_json::from_slice(
            &std::fs::read(manager.binding_control_store().path()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            map_after_first["slots"][0]["current"]["rex_execution"]["owner_pid"],
            owner0.owner_pid,
            "same-session recovery must persist the verified execution association"
        );

        let owner1 = spawn_codex_caller(&root, "execution-owner-1");
        std::env::set_var("COLLAB_TEST_REX_OWNER_PID", owner1.owner_pid.to_string());
        let request1 = IdentityContextRequest {
            operation_id: "rex-execution-s1".into(),
            action: "context".into(),
            invocation: "automatic".into(),
            facts: IdentityFacts {
                rex: Some(crate::proto::RexCandidate {
                    server: format!("unix://{socket_path}"),
                    session_id: "session:11111111-1111-1111-1111-111111111111".into(),
                    block_id: "block:22222222-2222-2222-2222-222222222222".into(),
                    cwd: root.display().to_string(),
                }),
                session_id: Some("session-s1".into()),
                caller_pid: Some(owner1.caller_pid),
                ..IdentityFacts::default()
            },
            invocation_cwd: Some(root.display().to_string()),
            approval: None,
            grant_approval: None,
            query: false,
        };
        let (_, second) = tokio::task::spawn_blocking({
            let manager = manager.clone();
            let context = context.clone();
            let request = request1.clone();
            move || manager.mapped_context(context, request)
        })
        .await
        .unwrap();
        assert!(
            second.ok,
            "a newer foreground execution must continue while the older execution is still alive: {second:?}"
        );
        let map_while_old_alive: serde_json::Value = serde_json::from_slice(
            &std::fs::read(manager.binding_control_store().path()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            map_while_old_alive["slots"][0]["current"]["binding"]["session_id"],
            "session-s1"
        );
        assert_eq!(
            map_while_old_alive["slots"][0]["current"]["rex_execution"]["owner_pid"],
            owner1.owner_pid
        );

        std::env::set_var("COLLAB_TEST_REX_OWNER_PID", owner0.owner_pid.to_string());
        let (_, older) = tokio::task::spawn_blocking({
            let manager = manager.clone();
            let context = context.clone();
            let mut request = request0.clone();
            request.operation_id = "rex-older-execution-return".into();
            move || manager.mapped_context(context, request)
        })
        .await
        .unwrap();
        assert!(!older.ok, "an older foreground execution must not reclaim the name: {older:?}");
        assert!(
            older.error.as_deref().is_some_and(|error| error.contains("older than the current Rex execution")),
            "the refusal must identify the older execution: {older:?}"
        );
        let after_older = manager.binding_control_store().load().unwrap();
        let current = after_older.active_values()
            .find(|value| value.binding.agent_id.as_str() == "rex-execution-owner")
            .unwrap();
        assert_eq!(current.binding.session_id.as_ref().unwrap().as_str(), "session-s1");
        assert_eq!(current.rex_execution.as_ref().unwrap().owner_pid, owner1.owner_pid);
        std::env::set_var("COLLAB_TEST_REX_OWNER_PID", owner1.owner_pid.to_string());
        let request1_retry = IdentityContextRequest {
            operation_id: "rex-execution-s1-retry".into(),
            ..request1.clone()
        };
        let (_, continued) = tokio::task::spawn_blocking({
            let manager = manager.clone();
            let context = context.clone();
            move || manager.mapped_context(context, request1_retry)
        })
        .await
        .unwrap();
        assert!(
            continued.ok,
            "the same current Rex execution must continue its name: {continued:?}"
        );
        let map: serde_json::Value = serde_json::from_slice(
            &std::fs::read(manager.binding_control_store().path()).unwrap(),
        )
        .unwrap();
        let slot = map["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|slot| {
                slot["current"]["binding"]["agent_id"] == "rex-execution-owner"
                    && slot["current"]["binding"]["session_id"] == "session-s1"
            })
            .expect("continued stable name");
        assert_eq!(
            slot["current"]["binding"]["session_id"],
            "session-s1",
            "current map did not advance: {map:#}"
        );
        assert_eq!(slot["current"]["rex_execution"]["owner_pid"], owner1.owner_pid);
        assert!(slot["current"]["rex_execution"]["process_start_id"].is_string());

        let stale_request = IdentityContextRequest {
            operation_id: "rex-execution-old-session-replay".into(),
            facts: IdentityFacts {
                session_id: Some("session-s0".into()),
                caller_pid: Some(owner1.caller_pid),
                ..request0.facts.clone()
            },
            ..request1.clone()
        };
        let (_, stale) = tokio::task::spawn_blocking({
            let manager = manager.clone();
            let context = context.clone();
            move || manager.mapped_context(context, stale_request)
        })
        .await
        .unwrap();
        assert!(
            !stale.ok,
            "an old session replayed by the new execution must not roll the current name back: {stale:?}"
        );
        assert!(
            stale.error.as_deref().is_some_and(|error| {
                error.contains("same Rex execution cannot prove a new agent session")
            }),
            "same-execution session change must retain its specific missing fact: {stale:?}"
        );
        let map_after_replay: serde_json::Value = serde_json::from_slice(
            &std::fs::read(manager.binding_control_store().path()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            map_after_replay["slots"][0]["current"]["binding"]["session_id"],
            "session-s1"
        );
        assert_eq!(
            map_after_replay["slots"][0]["current"]["rex_execution"]["owner_pid"],
            owner1.owner_pid
        );

        let current_control = manager.binding_control_store().load().unwrap();
        let target_key = current_control.active_values()
            .find(|value| value.binding.session_id.as_ref().is_some_and(|session| session.as_str() == "session-s1"))
            .and_then(|value| current_control.key_for_worker(&context.project_scope, &context.app_scope_id, value.binding.agent_id.as_str()).unwrap())
            .cloned()
            .unwrap();
        let target_name = target_key.name.clone();
        std::env::set_var("COLLAB_TEST_REX_OWNER_PID", owner0.owner_pid.to_string());
        let (_, human_restored) = tokio::task::spawn_blocking({
            let manager = manager.clone();
            let context = context.clone();
            let mut request = request0.clone();
            request.operation_id = "rex-human-restores-earlier-execution".into();
            request.grant_approval = Some(json!({"action":"restore_name", "requested_name":target_name,
                "authorization":"user restores this earlier running session"}));
            move || manager.mapped_context(context, request)
        }).await.unwrap();
        assert!(human_restored.ok, "explicit human restoration must commit: {human_restored:?}");
        let (_, associated) = tokio::task::spawn_blocking({
            let manager = manager.clone();
            let context = context.clone();
            let mut request = request0.clone();
            request.operation_id = "rex-human-current-observes-execution".into();
            move || manager.mapped_context(context, request)
        }).await.unwrap();
        assert!(associated.ok, "the restored current can establish its execution: {associated:?}");
        let initial_fence = manager.binding_control_store().load().unwrap()
            .current_for_name(&context.project_scope, &context.app_scope_id, &target_name).unwrap().unwrap()
            .active_binding().unwrap().rex_execution_fence.clone();
        let (_, refreshed) = tokio::task::spawn_blocking({
            let manager = manager.clone(); let context = context.clone(); let mut request = request0.clone();
            request.operation_id = "rex-current-refresh-preserves-fence".into();
            move || manager.mapped_context(context, request)
        }).await.unwrap();
        assert!(refreshed.ok, "{refreshed:?}");
        assert_eq!(manager.binding_control_store().load().unwrap()
            .current_for_name(&context.project_scope, &context.app_scope_id, &target_name).unwrap().unwrap()
            .active_binding().unwrap().rex_execution_fence, initial_fence);

        // A genuinely future execution remains admissible while both earlier
        // processes are alive. Restoring E0 again must retain the larger E2 fence.
        let owner2 = spawn_codex_caller(&root, "execution-owner-2");
        std::env::set_var("COLLAB_TEST_REX_OWNER_PID", owner2.owner_pid.to_string());
        let mut request2 = request0.clone();
        request2.facts.session_id = Some("session-s2".into());
        request2.facts.caller_pid = Some(owner2.caller_pid);
        request2.operation_id = "rex-future-after-human-fence".into();
        let (_, future) = tokio::task::spawn_blocking({
            let manager = manager.clone(); let context = context.clone();
            move || manager.mapped_context(context, request2)
        }).await.unwrap();
        assert!(future.ok, "future execution remains automatic: {future:?}");
        assert_eq!(future.data["result"]["snapshot"]["name"], target_name);
        std::env::set_var("COLLAB_TEST_REX_OWNER_PID", owner0.owner_pid.to_string());
        let (_, restored_again) = tokio::task::spawn_blocking({
            let manager = manager.clone(); let context = context.clone(); let mut request = request0.clone();
            request.operation_id = "rex-human-restores-earlier-execution-again".into();
            request.grant_approval = Some(json!({"action":"restore_name", "requested_name":target_name,
                "authorization":"user restores earlier running session again"}));
            move || manager.mapped_context(context, request)
        }).await.unwrap();
        assert!(restored_again.ok, "{restored_again:?}");
        let (_, associated_again) = tokio::task::spawn_blocking({
            let manager = manager.clone(); let context = context.clone(); let mut request = request0.clone();
            request.operation_id = "rex-human-current-associated-again".into();
            move || manager.mapped_context(context, request)
        }).await.unwrap();
        assert!(associated_again.ok, "{associated_again:?}");
        std::env::set_var("COLLAB_TEST_REX_OWNER_PID", owner1.owner_pid.to_string());
        let (_, displaced_return) = tokio::task::spawn_blocking({
            let manager = manager.clone();
            let context = context.clone();
            let mut request = request1.clone();
            request.operation_id = "rex-displaced-later-execution-returns".into();
            move || manager.mapped_context(context, request)
        }).await.unwrap();
        assert!(displaced_return.ok, "the verified displaced caller must register normally: {displaced_return:?}");
        let returned_name = displaced_return.data["result"]["snapshot"]["name"].as_str().unwrap();
        assert_ne!(returned_name, target_name, "ordinary registration must not reclaim the human-selected target");
        let control = manager.binding_control_store().load().unwrap();
        let target_slot = control.slot(&target_key).unwrap();
        assert_eq!(
            target_slot.identity_status(),
            crate::server::binding_slot::IdentityStatus::Unregistered
        );
        let target_previous = target_slot
            .reference_previous()
            .expect("the displaced target binding is preserved");
        assert_eq!(
            target_previous.binding.session_id.as_ref().unwrap().as_str(),
            "session-s0"
        );
        assert_eq!(
            target_previous.rex_execution.as_ref().unwrap().owner_pid,
            owner0.owner_pid
        );
        let returned = control
            .current_for_name(
                &context.project_scope,
                &context.app_scope_id,
                returned_name,
            )
            .unwrap()
            .unwrap()
            .active_binding()
            .unwrap();
        assert_eq!(returned.binding.session_id.as_ref().unwrap().as_str(), "session-s1");
        assert_eq!(returned.transport.kind, TransportKind::Rex);
        assert_eq!(
            returned.transport.rex_endpoint,
            target_previous.transport.rex_endpoint
        );
        assert_eq!(
            control
                .active_values()
                .filter(|value| {
                    value.transport.kind == TransportKind::Rex
                        && value.transport.rex_endpoint == returned.transport.rex_endpoint
                })
                .count(),
            1,
            "the later successful publication is the block's only current owner"
        );
        assert_eq!(control.lookup(&context.project_scope, &context.app_scope_id, &request1.facts, None)
            .unwrap().unwrap().identity_key.name, returned_name);
        let (_, returned_again) = tokio::task::spawn_blocking({
            let manager = manager.clone(); let context = context.clone(); let mut request = request1.clone();
            request.operation_id = "rex-displaced-ordinary-name-usable".into();
            move || manager.mapped_context(context, request)
        }).await.unwrap();
        assert!(returned_again.ok, "ordinary name must remain usable: {returned_again:?}");
        assert_eq!(returned_again.data["result"]["snapshot"]["name"], returned_name);
        let final_control = manager.binding_control_store().load().unwrap();
        assert!(
            final_control
                .current_for_name(&context.project_scope, &context.app_scope_id, &target_name)
                .unwrap()
                .is_none(),
            "the target name stays retired after the block's newer owner publishes"
        );
        assert_eq!(
            final_control
                .current_for_name(
                    &context.project_scope,
                    &context.app_scope_id,
                    returned_name,
                )
                .unwrap()
                .unwrap()
                .active_binding()
                .unwrap(),
            returned,
            "ordinary context continues the block's current owner"
        );
        owner2.stop();
        owner0.stop();
        owner1.stop();
        drop(listener);
        std::env::remove_var("COLLAB_TEST_REX_OWNER_PID");
        std::env::remove_var("COLLAB_TEST_REX_OWNER_NAME");
        std::env::remove_var("REXPRIMITIVE_LOG_DIR");
        set_test_cli(None);
        set_test_timeout(None);
        let _ = std::fs::remove_file(socket_path);
        std::fs::remove_dir_all(root).unwrap();
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
