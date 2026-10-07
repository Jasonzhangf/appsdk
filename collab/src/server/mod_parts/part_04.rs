include!("runtime_manager_setup.rs");

impl ProjectRuntimeManager {

    fn runtimes(&self) -> Vec<Arc<Server>> {
        let mut result = Vec::new();
        let routes = self.routes.lock().unwrap();
        for route in routes.values() {
            if let Some(runtime) = &route.runtime {
                // Multiple app scopes may intentionally share the resident
                // reducer.  The scheduler owns a runtime, not a route key;
                // ticking once per key would duplicate wakeups and could
                // consume a timer twice in the same interval.
                if result.iter().any(|existing| Arc::ptr_eq(existing, runtime)) {
                    continue;
                }
                result.push(runtime.clone());
            }
        }
        if !result
            .iter()
            .any(|runtime| Arc::ptr_eq(runtime, &self.host))
        {
            result.push(self.host.clone());
        }
        result
    }

    fn resolve_route_by_native_thread(
        &self,
        session_id: &str,
        native_thread_id: &str,
    ) -> Result<RouteResolution, String> {
        self.resolve_route_by_address(session_id, native_thread_id, None)
    }

    fn resolve_route_by_tmux_endpoint(
        &self,
        endpoint: &crate::proto::TmuxEndpoint,
    ) -> Result<RouteResolution, String> {
        crate::client::adapters::tmux::validate_endpoint(endpoint)?;
        self.resolve_route_by_address(&endpoint.tmux_session_id, &endpoint.pane_id, Some(endpoint))
    }

    fn resolve_staged_pane_recovery(
        &self,
        endpoint: &crate::proto::TmuxEndpoint,
        worker_id: &str,
        token: &str,
    ) -> Result<RouteResolution, String> {
        crate::client::adapters::tmux::validate_endpoint(endpoint)?;
        let old = self.host.state.lock().unwrap().global
            .lookup_unique_tmux_pane_route(endpoint).cloned()
            .ok_or_else(|| "RECOVERY_RECONCILE_REQUIRED: no unique host pane route".to_owned())?;
        if old.agent_id.as_str() != worker_id || old.tmux_endpoint.as_ref()
            .is_none_or(|bound| !crate::client::adapters::tmux::same_pane_route(bound, endpoint))
        {
            return Err("IDENTITY_RESTORE_CONFLICT: pane route belongs to another worker".into());
        }
        let key = (old.app_scope_id.as_str().to_owned(), old.project_scope.as_str().to_owned());
        let (runtime, storage_root) = {
            let routes = self.routes.lock().unwrap();
            let route = routes.get(&key)
                .ok_or_else(|| "RECOVERY_RECONCILE_REQUIRED: project route is missing".to_owned())?;
            (route.runtime.clone().ok_or_else(|| "RECOVERY_RECONCILE_REQUIRED: project runtime is not loaded".to_owned())?, route.storage_root.clone())
        };
        if Arc::ptr_eq(&runtime, &self.host) {
            return Err("RECOVERY_RECONCILE_REQUIRED: resident route has no split journal".into());
        }
        let pending = self.pending_same_pane_master_bindings(&runtime);
        let staged = pending.iter().any(|binding| {
            binding.same_principal(&old)
                && old.binding_id == binding.binding_id
                && old.runtime_id == binding.runtime_id
                && old.endpoint_generation.checked_add(1) == Some(binding.endpoint_generation)
                && binding.tmux_endpoint.as_ref().is_some_and(|new_endpoint|
                    crate::client::adapters::tmux::same_pane_route(new_endpoint, endpoint))
        });
        if !staged {
            return Err("RECOVERY_RECONCILE_REQUIRED: no authenticated adjacent project transition".into());
        }
        if verify(&runtime.state.lock().unwrap(), worker_id, token).is_err() {
            return Err("TOKEN_MISMATCH: pane recovery credential does not own worker".into());
        }
        match crate::client::adapters::tmux::probe(endpoint)
            .map_err(|error| format!("ROUTE_RESOLVE_UNKNOWN: {error}"))? {
            crate::client::adapters::tmux::PanePresence::Present => {}
            crate::client::adapters::tmux::PanePresence::Missing => return Err("ROUTE_RESOLVE_NOT_FOUND: tmux pane is gone".into()),
            crate::client::adapters::tmux::PanePresence::Unknown => return Err("ROUTE_RESOLVE_UNKNOWN: tmux pane liveness is uncertain".into()),
        }
        let route = RouteResolution {
            app_scope_id: old.app_scope_id,
            project_scope: old.project_scope.clone(),
            canonical_root: old.project_scope.as_str().to_owned(),
            storage_root: storage_root.to_string_lossy().into_owned(),
            agent_id: old.agent_id,
            binding_id: old.binding_id,
            endpoint_generation: old.endpoint_generation,
            session_id: old.session_id.ok_or_else(|| "ROUTE_RESOLVE_INVALID: old route has no session".to_owned())?,
            native_thread_id: old.native_thread_id.ok_or_else(|| "ROUTE_RESOLVE_INVALID: old route has no thread".to_owned())?,
        };
        route.validate().map_err(|error| format!("ROUTE_RESOLVE_INVALID: {error}"))?;
        Ok(route)
    }

    fn admit_committed_pane_register_retry(
        &self,
        context: &mut ProjectContext,
        req: &Req,
    ) -> Result<(), String> {
        let (Req::Register { worker_id, token, candidates: Some(candidates), .. }, Some(previous)) =
            (req, context.runtime_context.as_ref()) else { return Ok(()); };
        if candidates.appserver.is_some() { return Ok(()); }
        let Some(candidate) = candidates.tmux.as_ref() else { return Ok(()); };
        let key = Self::route_key(context);
        let Some(runtime) = self.routes.lock().unwrap().get(&key).and_then(|route| route.runtime.clone()) else {
            return Ok(());
        };
        let pending = self.pending_same_pane_master_bindings(&runtime);
        let Some(binding) = pending.into_iter().find(|binding| {
            binding.agent_id.as_str() == worker_id
                && binding.project_scope == context.project_scope
                && binding.app_scope_id == previous.appserver_id
                && binding.agent_id == previous.agent_id
                && binding.runtime_id == previous.runtime_id
                && binding.binding_id == previous.binding_id
                && previous.endpoint_generation.checked_add(1) == Some(binding.endpoint_generation)
                && binding.tmux_endpoint.as_ref().is_some_and(|endpoint|
                    crate::client::adapters::tmux::same_pane_route(endpoint, &candidate.endpoint))
        }) else { return Ok(()); };
        if verify(&runtime.state.lock().unwrap(), worker_id, token).is_err() {
            return Err("TOKEN_MISMATCH: pane register retry credential does not own worker".into());
        }
        let host_route = self
            .host
            .state
            .lock()
            .unwrap()
            .global
            .lookup_unique_tmux_pane_route(&candidate.endpoint)
            .cloned();
        let host_matches = host_route.as_ref().is_some_and(|host|
            host == &binding ||
            (host.same_principal(&binding)
                && host.endpoint_generation == previous.endpoint_generation
                && host.session_id == previous.session_id
                && host.native_thread_id == previous.native_thread_id));
        if !host_matches {
            return Err("RECOVERY_RECONCILE_REQUIRED: host pane route does not match retry transition".into());
        }
        match crate::client::adapters::tmux::probe(&candidate.endpoint)
            .map_err(|error| format!("ROUTE_RESOLVE_UNKNOWN: {error}"))? {
            crate::client::adapters::tmux::PanePresence::Present => {}
            _ => return Err("ROUTE_RESOLVE_UNKNOWN: pane is not present for register retry".into()),
        }
        context.runtime_context = Some(crate::identity::RuntimeIdentity {
            agent_id: binding.agent_id,
            runtime_id: binding.runtime_id,
            appserver_id: binding.app_scope_id,
            endpoint_generation: binding.endpoint_generation,
            binding_id: binding.binding_id,
            session_id: binding.session_id,
            native_thread_id: binding.native_thread_id,
        });
        Ok(())
    }

    fn resolve_route_by_address(
        &self,
        session_id: &str,
        native_thread_id: &str,
        requested_tmux_endpoint: Option<&crate::proto::TmuxEndpoint>,
    ) -> Result<RouteResolution, String> {
        let session_id = crate::identity::SessionId::new(session_id.to_owned())
            .map_err(|error| format!("ROUTE_RESOLVE_INVALID: {error}"))?;
        let native_thread_id = NativeThreadId::new(native_thread_id.to_owned())
            .map_err(|error| format!("ROUTE_RESOLVE_INVALID: {error}"))?;
        // Strict dual-key lookup first; a durable thread-only binding is a
        // read-only compatibility fallback, never a selector among several
        // threads.  cwd is execution context and is not part of this decision.
        let (binding, legacy) = {
            let state = self.host.state.lock().unwrap();
            if let Some(requested) = requested_tmux_endpoint {
                if let Some(binding) = state.global.lookup_tmux_route(requested).cloned() {
                    (binding, false)
                } else if let Some(tombstone) = state.global.lookup_tmux_route_tombstone(requested)
                {
                    return Err(format!(
                        "SESSION_THREAD_BINDING_STALE: tmux endpoint was retired; reboundTo=({}, {}); recovery: re-run the caller with its current pane endpoint; never revive the old route or hand-edit the journal",
                        tombstone.rebound_to.session_id.as_ref().map(ToString::to_string).unwrap_or_default(),
                        tombstone.rebound_to.native_thread_id.as_ref().map(ToString::to_string).unwrap_or_default()
                    ));
                } else {
                    return Err(format!(
                        "ROUTE_RESOLVE_NOT_FOUND: no registered Collab route is bound to tmux socket {} session {} pane {}",
                        requested.socket_path, requested.tmux_session_id, requested.pane_id
                    ));
                }
            } else if let Some(binding) = state
                .global
                .lookup_current_thread_route(&session_id, &native_thread_id)
                .cloned()
            {
                (binding, false)
            } else if !state
                .global
                .legacy_thread_route_matches(&native_thread_id)
                .is_empty()
            {
                // A legacy record is only a fallback for a thread with no live
                // strict owner.  If the thread is already session-bound to
                // another identity, a request carrying a different session
                // must not be routed through the older project.
                if !state
                    .global
                    .strict_bindings_for_native_thread(&native_thread_id)
                    .is_empty()
                {
                    return Err(format!(
                        "ROUTE_RESOLVE_AMBIGUOUS: App Server thread {native_thread_id} already has a session-bound binding, so it is not resolvable under session {session_id}; run collab context with the current runtime facts; preserve the error if the daemon cannot reconcile the binding"
                    ));
                }
                let matches = state.global.legacy_thread_route_matches(&native_thread_id);
                if matches.len() > 1 {
                    return Err(format!(
                        "ROUTE_RESOLVE_AMBIGUOUS: App Server thread {native_thread_id} has {count} legacy thread-only bindings under app scope {app}; run collab context with the current runtime facts; preserve unresolved conflicts for the identity owner",
                        count = matches.len(),
                        app = matches[0].app_scope_id.as_str(),
                    ));
                }
                (matches[0].clone(), true)
            } else if let Some(tombstone) = state
                .global
                .lookup_current_thread_route_tombstone(&session_id, &native_thread_id)
            {
                return Err(format!(
                    "SESSION_THREAD_BINDING_STALE: old session/thread address ({session_id}, {native_thread_id}) was retired; reboundTo=({}, {}); recovery: re-run the caller with the current address; never revive the old route or hand-edit the journal",
                    tombstone
                        .rebound_to
                        .session_id
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                    tombstone
                        .rebound_to
                        .native_thread_id
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default()
                ));
            } else {
                return Err(format!(
                    "ROUTE_RESOLVE_NOT_FOUND: no registered Collab route is bound to App Server thread {native_thread_id} under session {session_id}; {ROUTE_RESOLVE_NOT_FOUND_RECOVERY}"
                ));
            }
        };
        let key = (
            binding.app_scope_id.as_str().to_owned(),
            binding.project_scope.as_str().to_owned(),
        );
        let (storage_root, runtime) = {
            let routes = self.routes.lock().unwrap();
            let route = routes.get(&key).ok_or_else(|| {
                format!(
                    "ROUTE_RESOLVE_STALE_INDEX: current route state for App Server thread {native_thread_id} references an unknown route"
                )
            })?;
            let runtime = route.runtime.clone().ok_or_else(|| {
                format!(
                    "ROUTE_RESOLVE_STALE_INDEX: current route state for App Server thread {native_thread_id} references an unavailable runtime"
                )
            })?;
            (route.storage_root.clone(), runtime)
        };
        // A legacy record has no persisted session; keep the host-provided
        // session as the live address without writing it back to the record.
        let session_id = match binding.session_id.clone() {
            Some(bound) => bound,
            None if legacy => session_id,
            None => {
                return Err(format!(
                    "ROUTE_RESOLVE_INVALID: current route state for App Server thread {native_thread_id} has no session id"
                ))
            }
        };
        let registered = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "ROUTE_RESOLVE_STALE_INDEX: current route state for App Server thread {native_thread_id} references a missing runtime binding"
                )
            })?;
        if registered != binding {
            return Err(format!(
                "ROUTE_RESOLVE_INVALID: current route state for App Server thread {native_thread_id} conflicts with its runtime binding"
            ));
        }
        let canonical_root = binding.project_scope.as_str().to_owned();
        let transport = {
            let state = runtime.state.lock().unwrap();
            state
                .workers
                .get(registered.agent_id.as_str())
                .and_then(selected_transport_for_worker)
                .ok_or_else(|| {
                    format!(
                        "ROUTE_RESOLVE_INVALID: current route state for App Server thread {native_thread_id} has no selected transport"
                    )
                })?
        };
        match transport.kind {
            TransportKind::Tmux => {
                let endpoint = transport.tmux_endpoint.as_ref().ok_or_else(|| {
                    format!("ROUTE_RESOLVE_INVALID: tmux pane {native_thread_id} has no endpoint")
                })?;
                if let Some(requested) = requested_tmux_endpoint {
                    if !crate::client::adapters::tmux::same_pane_route(requested, endpoint) {
                        return Err(format!(
                            "ROUTE_RESOLVE_NOT_FOUND: tmux endpoint does not match the current route for pane {native_thread_id}"
                        ));
                    }
                }
                match crate::client::adapters::tmux::probe(endpoint)
                    .map_err(|error| format!("ROUTE_RESOLVE_UNKNOWN: {error}"))?
                {
                    crate::client::adapters::tmux::PanePresence::Present => {}
                    crate::client::adapters::tmux::PanePresence::Missing => {
                        return Err(format!(
                            "ROUTE_RESOLVE_NOT_FOUND: tmux pane {native_thread_id} is gone"
                        ));
                    }
                    crate::client::adapters::tmux::PanePresence::Unknown => {
                        return Err(format!("ROUTE_RESOLVE_UNKNOWN: tmux pane {native_thread_id} liveness is uncertain"));
                    }
                }
            }
            TransportKind::AppServer => {
                if requested_tmux_endpoint.is_some() {
                    return Err(format!(
                        "ROUTE_RESOLVE_NOT_FOUND: pane {native_thread_id} is not registered with a tmux endpoint"
                    ));
                }
                let candidate = crate::proto::AppServerCandidate {
                    endpoint: transport.endpoint.ok_or_else(|| {
                        format!("ROUTE_RESOLVE_INVALID: current route state for thread {native_thread_id} has no endpoint")
                    })?,
                    namespace: transport.namespace.ok_or_else(|| {
                        format!("ROUTE_RESOLVE_INVALID: current route state for thread {native_thread_id} has no namespace")
                    })?,
                    session_id: session_id.as_str().to_owned(),
                    thread_id: native_thread_id.as_str().to_owned(),
                    cwd: canonical_root.clone(),
                };
                (runtime.appserver_candidate_check)(&candidate).map_err(|error| {
                    format!(
                        "ROUTE_RESOLVE_INVALID: App Server identity verification failed: {error}"
                    )
                })?;
            }
            TransportKind::Dsh => {
                // Re-verification for a dsh peer is a fresh single-use challenge
                // against the gateway, never a pane comparison: there is no pane.
                if requested_tmux_endpoint.is_some() {
                    return Err(format!(
                        "ROUTE_RESOLVE_NOT_FOUND: address {native_thread_id} is not registered with a tmux endpoint"
                    ));
                }
                let endpoint = transport.endpoint.ok_or_else(|| {
                    format!("ROUTE_RESOLVE_INVALID: current route state for dsh agent {native_thread_id} has no endpoint")
                })?;
                let runtime_id = transport.namespace.ok_or_else(|| {
                    format!("ROUTE_RESOLVE_INVALID: current route state for dsh agent {native_thread_id} has no gateway runtime id")
                })?;
                match crate::client::adapters::dsh::probe(
                    &endpoint,
                    &runtime_id,
                    native_thread_id.as_str(),
                ) {
                    crate::client::adapters::dsh::PeerPresence::Live => {}
                    crate::client::adapters::dsh::PeerPresence::Absent => {
                        return Err(format!(
                            "ROUTE_RESOLVE_NOT_FOUND: gateway does not know dsh agent {native_thread_id}"
                        ));
                    }
                    crate::client::adapters::dsh::PeerPresence::Unknown => {
                        return Err(format!(
                            "ROUTE_RESOLVE_UNKNOWN: dsh agent {native_thread_id} liveness is uncertain"
                        ));
                    }
                }
            }
        }
        let route = RouteResolution {
            app_scope_id: binding.app_scope_id,
            project_scope: binding.project_scope,
            canonical_root,
            storage_root: storage_root.to_string_lossy().into_owned(),
            agent_id: binding.agent_id,
            binding_id: binding.binding_id,
            endpoint_generation: binding.endpoint_generation,
            session_id,
            native_thread_id: binding.native_thread_id.ok_or_else(|| {
                "ROUTE_RESOLVE_INVALID: current route state has no native App Server thread"
                    .to_owned()
            })?,
        };
        route
            .validate()
            .map_err(|error| format!("ROUTE_RESOLVE_INVALID: {error}"))?;
        Ok(route)
    }

    fn commit_current_thread_route(
        &self,
        runtime: &Arc<Server>,
        context: &ProjectContext,
        worker_id: &str,
        previous_binding: Option<&RuntimeBinding>,
        previous_grant: Option<&crate::server::global_state::MasterGrant>,
        previous_worker: Option<&WorkerRec>,
        previous_subscriptions: &[NotificationSubscription],
    ) -> Result<(), String> {
        let route_scope = RouteScope {
            app_scope_id: context.app_scope_id.clone(),
            project_scope_id: context.project_scope.clone(),
        };
        let binding_id = BindingId::new(sanitize_identifier(&format!("binding-{worker_id}")))
            .map_err(|error| format!("ROUTE_TRANSITION_INVALID: {error}"))?;
        let binding = runtime
            .state
            .lock()
            .unwrap()
            .global
            .lookup_binding_for(&route_scope, &binding_id)
            .filter(|binding| binding.agent_id.as_str() == worker_id)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "ROUTE_TRANSITION_INVALID: successful registration for {worker_id} has no matching runtime binding"
                )
            })?;
        let native_thread_id = binding.native_thread_id.as_ref().ok_or_else(|| {
            format!(
                "ROUTE_TRANSITION_INVALID: successful registration for {worker_id} has no native App Server thread"
            )
        })?;
        let session_id = binding.session_id.as_ref().ok_or_else(|| {
            format!(
                "ROUTE_TRANSITION_INVALID: successful registration for {worker_id} has no session id"
            )
        })?;
        let current = {
            let state = self.host.state.lock().unwrap();
            let current = match binding.tmux_endpoint.as_ref() {
                Some(endpoint)
                    if endpoint.codex_session_id.is_none()
                        && endpoint.codex_thread_id.is_none() =>
                {
                    state.global.lookup_tmux_route(endpoint)
                }
                _ => state
                    .global
                    .lookup_current_thread_route(session_id, native_thread_id),
            };
            current.cloned()
        };
        if current.as_ref() == Some(&binding) {
            return Ok(());
        }
        if let Some(existing) = current {
            // A pane has one owner and the later registrant wins it. The host
            // index evicts every other claimant of the same pane inside this
            // same commit, so a same-pane incumbent is replaced here instead of
            // fencing the registration. Only a claim on a *different* pane stays
            // a conflict, because two workers must never share one App Server
            // thread.
            let same_pane = binding.tmux_endpoint.as_ref().is_some_and(|candidate| {
                existing
                    .tmux_endpoint
                    .as_ref()
                    .is_some_and(|previous| {
                        crate::client::adapters::tmux::same_owned_pane(previous, candidate)
                    })
            });
            if !same_pane
                && (existing.agent_id != binding.agent_id
                    || existing.route_scope() != binding.route_scope())
            {
                return Err(format!(
                    "RUNTIME_BINDING_REJECTED: App Server thread {} is already bound to worker {}",
                    native_thread_id, existing.agent_id
                ));
            }
        }
        #[cfg(test)]
        let injected_publish_error = self
            .fail_current_thread_route_publish
            .swap(false, std::sync::atomic::Ordering::SeqCst)
            .then(|| "injected current thread route publication failure".to_string());
        #[cfg(not(test))]
        let injected_publish_error: Option<String> = None;
        enum RoutePublishError {
            Definite(String),
            Ambiguous(String),
        }
        let publish_result = match injected_publish_error {
            Some(error) => Err(RoutePublishError::Definite(error)),
            None => self
                .host
                .commit_checked(&[Event::GlobalCurrentThreadRouteSet {
                    binding: binding.clone(),
                }])
                .map(|_| ())
                .map_err(|error| match error {
                    notification_contract::JournalError::Append(error) => {
                        RoutePublishError::Ambiguous(error)
                    }
                    notification_contract::JournalError::Flush(error) => {
                        RoutePublishError::Ambiguous(error)
                    }
                    notification_contract::JournalError::Replay(error) => {
                        RoutePublishError::Ambiguous(error)
                    }
                    notification_contract::JournalError::Reducer(error) => {
                        RoutePublishError::Ambiguous(error)
                    }
                    notification_contract::JournalError::InvalidCommand(error) => {
                        RoutePublishError::Ambiguous(error)
                    }
                }),
        };
        if let Err(RoutePublishError::Definite(error)) = publish_result {
            let rollback = runtime
                .commit_checked(&[Event::GlobalRuntimeBindingRollback {
                    failed: binding,
                    previous: previous_binding.cloned(),
                    previous_grant: previous_grant.cloned(),
                    previous_worker: previous_worker.cloned(),
                    previous_subscriptions: previous_subscriptions.to_vec(),
                }])
                .map_err(|rollback_error| {
                    format!(
                        "ROUTE_TRANSITION_DURABILITY_FAILED: {error}; rollback journal commit failed: {rollback_error}"
                    )
                });
            rollback?;
            return Err(format!(
                "ROUTE_TRANSITION_DURABILITY_FAILED: {error}; previous worker transport, notification subscriptions, runtime binding, and master grant were restored"
            ));
        }
        if let Err(RoutePublishError::Ambiguous(error)) = publish_result {
            return Err(format!(
                "ROUTE_TRANSITION_DURABILITY_FAILED: host journal publication outcome is unknown: {error}; registration route may already be durable; preserve both journals and restart the affected daemon from the reviewed AppSDK main binary before retrying; do not edit routes or bindings manually"
            ));
        }
        Ok(())
    }

    fn finalize_registration(
        &self,
        runtime: Arc<Server>,
        context: &ProjectContext,
        worker_id: Option<&str>,
        previous_binding: Option<RuntimeBinding>,
        previous_grant: Option<crate::server::global_state::MasterGrant>,
        previous_worker: Option<WorkerRec>,
        previous_subscriptions: Vec<NotificationSubscription>,
        response: Resp,
    ) -> (Arc<Server>, Resp) {
        let Some(worker_id) = worker_id.filter(|_| response.ok) else {
            return (runtime, response);
        };
        match self.commit_current_thread_route(
            &runtime,
            context,
            worker_id,
            previous_binding.as_ref(),
            previous_grant.as_ref(),
            previous_worker.as_ref(),
            &previous_subscriptions,
        ) {
            Ok(()) => (runtime, response),
            Err(error) => (
                runtime,
                Resp::err(format!(
                    "{error}; recovery: preserve the project and host journals, then restart the affected daemon from the reviewed AppSDK main binary so durable state can be replayed; do not edit routes or bindings manually"
                )),
            ),
        }
    }

    fn registration_rollback_state(
        &self,
        runtime: &Arc<Server>,
        context: &ProjectContext,
        worker_id: &str,
    ) -> Result<
        (
            Option<RuntimeBinding>,
            Option<crate::server::global_state::MasterGrant>,
            Option<WorkerRec>,
            Vec<NotificationSubscription>,
        ),
        String,
    > {
        let route_scope = RouteScope {
            app_scope_id: context.app_scope_id.clone(),
            project_scope_id: context.project_scope.clone(),
        };
        let binding_id = BindingId::new(sanitize_identifier(&format!("binding-{worker_id}")))
            .map_err(|error| format!("ROUTE_TRANSITION_INVALID: {error}"))?;
        let state = runtime.state.lock().unwrap();
        let mut previous_subscriptions = state
            .notification_subscriptions
            .values()
            .filter(|subscription| subscription.worker_id == worker_id)
            .cloned()
            .collect::<Vec<_>>();
        previous_subscriptions.sort_by(|left, right| left.id.cmp(&right.id));
        Ok((
            state
                .global
                .lookup_binding_for(&route_scope, &binding_id)
                .filter(|binding| binding.agent_id.as_str() == worker_id)
                .cloned(),
            state
                .global
                .lookup_master_grant_for(&route_scope, &binding_id)
                .cloned(),
            state.workers.get(worker_id).cloned(),
            previous_subscriptions,
        ))
    }

    fn route_key(context: &ProjectContext) -> RouteKey {
        (
            context.app_scope_id.as_str().to_owned(),
            context.project_scope.as_str().to_owned(),
        )
    }

    fn has_project_route(&self, project_scope: &str) -> bool {
        self.routes
            .lock()
            .unwrap()
            .keys()
            .any(|(_, project)| project == project_scope)
    }

    fn storage_root_for_new(&self, root: &Path, project_scope: &str, app_scope: &str) -> PathBuf {
        if self.has_project_route(project_scope) {
            app_scope_storage_path(root, app_scope)
        } else {
            root.to_path_buf()
        }
    }

    fn runtime_init_gate(&self, key: &RouteKey) -> Arc<Mutex<()>> {
        self.runtime_init_gates
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Open and install a pending route exactly once. The double-check after
    /// acquiring the per-route gate is required because Poll and blocking
    /// requests can select the same durable pending route concurrently.
    fn ensure_runtime(
        &self,
        key: &RouteKey,
        root: &Path,
        storage_root: &Path,
    ) -> Result<Arc<Server>, String> {
        let gate = self.runtime_init_gate(key);
        let _guard = gate.lock().unwrap();
        if let Some(runtime) = self
            .routes
            .lock()
            .unwrap()
            .get(key)
            .and_then(|route| route.runtime.clone())
        {
            return Ok(runtime);
        }
        let (runtime, project_lock) = self.build_runtime(root, storage_root)?;
        self.install_runtime(key, runtime.clone(), project_lock);
        Ok(runtime)
    }

    fn build_runtime(
        &self,
        root: &Path,
        storage_root: &Path,
    ) -> Result<(Arc<Server>, Option<std::fs::File>), String> {
        let root = std::fs::canonicalize(root).map_err(|error| {
            format!("PROJECT_ROUTE_NOT_READY/UNSUPPORTED: canonicalize project root: {error}")
        })?;
        if !root.join(".agent-collab").is_dir() {
            return Err(format!(
                "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: project {} is not initialized for Collab",
                root.display()
            ));
        }
        let storage_root = validate_runtime_storage_root(
            &root,
            storage_root,
            "PROJECT_ROUTE_NOT_READY/UNSUPPORTED",
        )?;
        let server_dir = storage_root.join(".agent-collab").join("server");
        std::fs::create_dir_all(&server_dir).map_err(|error| {
            format!("PROJECT_ROUTE_NOT_READY/UNSUPPORTED: create runtime storage: {error}")
        })?;

        let project_lock = if root == self.host_root {
            None
        } else if self.project_locks.lock().unwrap().contains_key(&root) {
            None
        } else {
            Some(
                acquire_legacy_writer_lock(
                    &root.join(".agent-collab/server/daemon.lock"),
                    "project daemon",
                )
                .map_err(|error| format!("PROJECT_ROUTE_NOT_READY/UNSUPPORTED: {error}"))?,
            )
        };
        let journal_path = server_dir.join("journal.jsonl");
        let state = replay_from_journal(&root, &journal_path).map_err(|error| {
            format!(
                "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: runtime journal replay for {}: {error}",
                root.display()
            )
        })?;
        let journal_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&journal_path)
            .map_err(|error| {
                format!("PROJECT_ROUTE_NOT_READY/UNSUPPORTED: open runtime journal: {error}")
            })?;
        let runtime = Arc::new(Server {
            config: crate::config::load(&root).map_err(|error| {
                format!("PROJECT_ROUTE_NOT_READY/UNSUPPORTED: load project config: {error}")
            })?,
            root,
            storage_root,
            journal_path,
            host_paths: self.host.host_paths.clone(),
            state: Mutex::new(state),
            journal: Mutex::new(journal_file),
            appserver_candidate_check: self.host.appserver_candidate_check.clone(),
            #[cfg(not(test))]
            tmux_notification_sink: self.host.tmux_notification_sink.clone(),
            appserver_notification_sink: self.host.appserver_notification_sink.clone(),
            appserver_thread_status: self.host.appserver_thread_status.clone(),
            appserver_thread_archive: self.host.appserver_thread_archive.clone(),
            mailbox_notify: Notify::new(),
        });
        restore_registered_peer_default_leases(&runtime);
        purge_expired_storage(&runtime, now_ms());
        Ok((runtime, project_lock))
    }

    fn install_runtime(
        &self,
        key: &RouteKey,
        runtime: Arc<Server>,
        project_lock: Option<std::fs::File>,
    ) {
        if let Some(lock) = project_lock {
            self.project_locks
                .lock()
                .unwrap()
                .insert(runtime.root.clone(), lock);
        }
        self.routes
            .lock()
            .unwrap()
            .entry(key.clone())
            .and_modify(|route| route.runtime = Some(runtime.clone()))
            .or_insert(RuntimeRoute {
                root: runtime.root.clone(),
                storage_root: runtime.storage_root.clone(),
                runtime: Some(runtime),
            });
    }

    fn install_pending_route(&self, key: &RouteKey, root: &Path, storage_root: &Path) {
        self.routes
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_insert_with(|| RuntimeRoute {
                root: root.to_path_buf(),
                storage_root: storage_root.to_path_buf(),
                runtime: None,
            });
    }

    fn storage_owner(
        &self,
        storage_root: &Path,
        records: &[HostRouteRecord],
    ) -> Result<Option<String>, String> {
        if storage_roots_equal(&self.host.root, storage_root)?
            || storage_roots_equal(&self.host.storage_root, storage_root)?
        {
            return Ok(Some("resident host".into()));
        }
        let route_owner = {
            let routes = self.routes.lock().unwrap();
            let mut owner = None;
            for (key, route) in routes.iter() {
                if storage_roots_equal(&route.storage_root, storage_root)? {
                    owner = Some(format!("route ({}, {})", key.0, key.1));
                    break;
                }
            }
            owner
        };
        if route_owner.is_some() {
            return Ok(route_owner);
        }
        for record in records {
            if storage_roots_equal(Path::new(&record.storage_root), storage_root)? {
                return Ok(Some(format!(
                    "route ({}, {})",
                    record.app_scope_id, record.project_scope
                )));
            }
        }
        Ok(None)
    }

    fn append_route_record(
        &self,
        context: &ProjectContext,
        storage_root: &Path,
    ) -> Result<(), String> {
        let project_root = Path::new(&context.canonical_root);
        let storage_root = validate_runtime_storage_root(
            project_root,
            storage_root,
            "HOST_ROUTE_DURABILITY_FAILED",
        )?;
        self.append_route_record_validated(context, &storage_root, false)
    }

    fn append_resident_route_record(&self, context: &ProjectContext) -> Result<(), String> {
        let project_root = Path::new(&context.canonical_root);
        if !storage_roots_equal(project_root, &self.host_root)
            .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?
        {
            return Err(
                "HOST_ROUTE_DURABILITY_FAILED: resident route root does not match host root".into(),
            );
        }
        let storage_root = storage_owner_path(&self.host.storage_root)
            .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?;
        self.append_route_record_validated(context, &storage_root, true)
    }

    fn append_route_record_validated(
        &self,
        context: &ProjectContext,
        storage_root: &Path,
        allow_resident_storage: bool,
    ) -> Result<(), String> {
        if allow_resident_storage {
            let project_root = Path::new(&context.canonical_root);
            let resident_root = storage_roots_equal(project_root, &self.host_root)
                .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?;
            let resident_storage = storage_roots_equal(storage_root, &self.host.storage_root)
                .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?;
            if !resident_root || !resident_storage {
                return Err(
                    "HOST_ROUTE_DURABILITY_FAILED: resident storage exception does not match host"
                        .into(),
                );
            }
        }
        let record = HostRouteRecord {
            version: 1,
            op: "register".into(),
            app_scope_id: context.app_scope_id.as_str().into(),
            project_scope: context.project_scope.as_str().into(),
            canonical_root: context.canonical_root.clone(),
            storage_root: storage_root.to_string_lossy().into_owned(),
            registered_ms: now_ms(),
        };
        let existing_records = load_host_route_records(&self.route_journal).map_err(|error| {
            format!("HOST_ROUTE_DURABILITY_FAILED: validate route journal: {error}")
        })?;
        if let Some(owner) = self
            .storage_owner(&storage_root, &existing_records)
            .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?
            .filter(|_| !allow_resident_storage)
        {
            return Err(format!(
                "HOST_ROUTE_DURABILITY_FAILED: runtime storage root {} is already owned by {}",
                record.storage_root, owner
            ));
        }
        append_host_route_record(&self.route_journal, &record)
    }

    fn select_runtime(&self, context: &ProjectContext) -> Result<Arc<Server>, String> {
        context
            .validate()
            .map_err(|error| format!("PROJECT_CONTEXT_INVALID: {error}"))?;
        let key = Self::route_key(context);
        let pending = {
            let routes = self.routes.lock().unwrap();
            let Some(route) = routes.get(&key) else {
                return Err(format!(
                    "PROJECT_SCOPE_UNKNOWN: no host route is registered for {}",
                    context.canonical_root
                ));
            };
            if let Some(runtime) = &route.runtime {
                return Ok(runtime.clone());
            }
            (route.root.clone(), route.storage_root.clone())
        };
        self.ensure_runtime(&key, &pending.0, &pending.1)
    }

    fn verify_cross_project_source(
        &self,
        from: &str,
        from_project: &str,
        assigned_by: &str,
        approval: Option<&str>,
        assigned_ms: i64,
    ) -> Result<String, String> {
        let source_root = std::fs::canonicalize(from_project).map_err(|error| {
            format!("CROSS_PROJECT_SOURCE_REJECTED: canonicalize source project: {error}")
        })?;
        if !source_root.join(".agent-collab").is_dir() {
            return Err(format!(
                "CROSS_PROJECT_SOURCE_REJECTED: source project {} is not initialized for Collab",
                source_root.display()
            ));
        }
        let source_scope = GlobalState::canonical_project_scope(&source_root)
            .map_err(|error| format!("CROSS_PROJECT_SOURCE_REJECTED: {error}"))?;
        let pending = self
            .routes
            .lock()
            .unwrap()
            .iter()
            .filter(|((_, project_scope), _)| project_scope == source_scope.as_str())
            .map(|(key, route)| (key.clone(), route.root.clone(), route.storage_root.clone()))
            .collect::<Vec<_>>();
        if pending.is_empty() {
            return Err(format!(
                "CROSS_PROJECT_SOURCE_REJECTED: no registered source route for {}",
                source_scope.as_str()
            ));
        }

        let mut matches = Vec::new();
        for (key, root, storage_root) in pending {
            let runtime = self
                .ensure_runtime(&key, &root, &storage_root)
                .map_err(|error| format!("CROSS_PROJECT_SOURCE_REJECTED: {error}"))?;
            let state = runtime.state.lock().unwrap();
            let live_master = match live_master_id(&runtime, &state) {
                Ok(master) => master,
                Err(error) => {
                    return Err(format!("CROSS_PROJECT_SOURCE_REJECTED: {error}"));
                }
            };
            if live_master.as_deref() != Some(from) {
                continue;
            }
            let route_scope = match server_route_scope(&runtime, &state) {
                Ok(Some(route_scope)) => route_scope,
                Ok(None) => continue,
                Err(error) => {
                    return Err(format!("CROSS_PROJECT_SOURCE_REJECTED: {error}"));
                }
            };
            let grant = current_master_grant(&state, Some(&route_scope));
            if grant.as_ref().map(|grant| grant.granted_by.as_str()) != Some(assigned_by)
                || grant.as_ref().map(|grant| grant.approval.as_str()) != approval
                || grant.as_ref().map(|grant| grant.granted_at_ms) != Some(assigned_ms)
            {
                return Err(
                    "CROSS_PROJECT_SOURCE_REJECTED: source master assignment evidence does not match the source reducer"
                        .into(),
                );
            }
            let source_thread_id = state
                .workers
                .get(from)
                .and_then(selected_transport_for_worker)
                .and_then(|transport| transport.thread_id)
                .ok_or_else(|| {
                    "CROSS_PROJECT_SOURCE_REJECTED: source master has no live App Server thread"
                        .to_string()
                })?;
            matches.push((key, source_thread_id));
        }

        match matches.len() {
            1 => Ok(matches
                .pop()
                .expect("one source match")
                .1),
            0 => Err(
                "CROSS_PROJECT_SOURCE_REJECTED: sender is not the live master of the source project"
                    .into(),
            ),
            _ => Err(
                "CROSS_PROJECT_SOURCE_REJECTED: source master route is ambiguous; use one registered source app scope"
                    .into(),
            ),
        }
    }

    fn dispatch_cross_project_send(
        &self,
        target_context: &ProjectContext,
        req: Req,
    ) -> (Arc<Server>, Resp) {
        let Req::CrossProjectSend {
            from,
            from_project,
            source_master_assigned_by,
            source_master_approval,
            source_master_assigned_ms,
            to,
            subject,
            body,
            in_reply_to,
        } = req
        else {
            unreachable!("cross-project dispatch requires CrossProjectSend");
        };
        let target = match self.select_runtime(target_context) {
            Ok(runtime) => runtime,
            Err(error) => return (self.host.clone(), Resp::err(error)),
        };
        let source_thread_id = match self.verify_cross_project_source(
            &from,
            &from_project,
            &source_master_assigned_by,
            source_master_approval.as_deref(),
            source_master_assigned_ms,
        ) {
            Ok(source_thread_id) => source_thread_id,
            Err(error) => return (target, Resp::err(error)),
        };
        let response = handle_cross_project_send(
            &target,
            from,
            from_project,
            source_thread_id,
            source_master_assigned_by,
            source_master_approval,
            source_master_assigned_ms,
            to,
            subject,
            body,
            in_reply_to,
        );
        (target, response)
    }

    fn dispatch_sync(
        &self,
        project_context: Option<ProjectContext>,
        req: Req,
    ) -> (Arc<Server>, Resp) {
        let Some(mut context) = project_context else {
            if matches!(req, Req::Ping) {
                let response = dispatch_with_route_context(&self.host, req, None);
                return (self.host.clone(), response);
            }
            return (
                self.host.clone(),
                Resp::err(
                    "PROJECT_CONTEXT_REQUIRED: canonical project root and scope are required",
                ),
            );
        };
        if let Err(error) = context.validate() {
            return (
                self.host.clone(),
                Resp::err(format!("PROJECT_CONTEXT_INVALID: {error}")),
            );
        }
        if let Req::IdentityContext { facts } = req {
            return self.identity_context(context, facts);
        }
        let key = Self::route_key(&context);
        let is_register = matches!(req, Req::Register { .. });
        let register_worker_id = match &req {
            Req::Register { worker_id, .. } => Some(worker_id.clone()),
            _ => None,
        };
        let _register_guard = is_register.then(|| self.register_gate.lock().unwrap());
        if let Err(error) = self.admit_committed_pane_register_retry(&mut context, &req) {
            return (self.host.clone(), Resp::err(error));
        }
        let superseded = match self.validate_current_thread_candidate(&context, &req) {
            Ok(superseded) => superseded,
            Err(error) => return (self.host.clone(), Resp::err(error)),
        };
        if matches!(req, Req::CrossProjectSend { .. }) {
            return self.dispatch_cross_project_send(&context, req);
        }

        if let Some((runtime, _)) = self.routes.lock().unwrap().get(&key).and_then(|route| {
            route
                .runtime
                .as_ref()
                .map(|runtime| (runtime.clone(), false))
        }) {
            if !is_register {
                if let Err(error) = self.same_pane_master_route_ready(&runtime) {
                    return (runtime, Resp::err(error));
                }
            }
            let rollback_state = match register_worker_id.as_deref() {
                Some(worker_id) => {
                    match self.registration_rollback_state(&runtime, &context, worker_id) {
                        Ok(state) => state,
                        Err(error) => return (runtime, Resp::err(error)),
                    }
                }
                None => (None, None, None, Vec::new()),
            };
            let response =
                if let Err(error) = validate_request_context(&runtime, &req, Some(&context)) {
                    Resp::err(error)
                } else {
                    dispatch_with_route_context(&runtime, req, Some(context.clone()))
                };
            let (runtime, response) = self.finalize_registration(
                runtime,
                &context,
                register_worker_id.as_deref(),
                rollback_state.0,
                rollback_state.1,
                rollback_state.2,
                rollback_state.3,
                response,
            );
            return (runtime, self.retire_superseded_claimants(superseded, response));
        }

        let pending_route = self
            .routes
            .lock()
            .unwrap()
            .get(&key)
            .map(|route| (route.root.clone(), route.storage_root.clone()));
        if let Some((root, storage_root)) = pending_route {
            let runtime = match self.ensure_runtime(&key, &root, &storage_root) {
                Ok(runtime) => runtime,
                Err(error) => return (self.host.clone(), Resp::err(error)),
            };
            if !is_register {
                if let Err(error) = self.same_pane_master_route_ready(&runtime) {
                    return (runtime, Resp::err(error));
                }
            }
            let rollback_state = match register_worker_id.as_deref() {
                Some(worker_id) => {
                    match self.registration_rollback_state(&runtime, &context, worker_id) {
                        Ok(state) => state,
                        Err(error) => return (runtime, Resp::err(error)),
                    }
                }
                None => (None, None, None, Vec::new()),
            };
            let response =
                if let Err(error) = validate_request_context(&runtime, &req, Some(&context)) {
                    Resp::err(error)
                } else {
                    dispatch_with_route_context(&runtime, req, Some(context.clone()))
                };
            let (runtime, response) = self.finalize_registration(
                runtime,
                &context,
                register_worker_id.as_deref(),
                rollback_state.0,
                rollback_state.1,
                rollback_state.2,
                rollback_state.3,
                response,
            );
            return (runtime, self.retire_superseded_claimants(superseded, response));
        }

        // The first route for the daemon's resident project keeps the
        // backwards-compatible resident reducer.  A second app scope gets a
        // separate runtime and storage namespace just like any other route.
        let context_root = PathBuf::from(&context.canonical_root);
        if context_root == self.host_root
            && !self.has_project_route(&context.project_scope.as_str())
        {
            if is_register {
                if let Err(error) = self.append_resident_route_record(&context) {
                    return (self.host.clone(), Resp::err(error));
                }
            }
            let rollback_state = match register_worker_id.as_deref() {
                Some(worker_id) => {
                    match self.registration_rollback_state(&self.host, &context, worker_id) {
                        Ok(state) => state,
                        Err(error) => return (self.host.clone(), Resp::err(error)),
                    }
                }
                None => (None, None, None, Vec::new()),
            };
            let response =
                if let Err(error) = validate_request_context(&self.host, &req, Some(&context)) {
                    Resp::err(error)
                } else {
                    dispatch_with_route_context(&self.host, req, Some(context.clone()))
                };
            if !response.ok {
                return (self.host.clone(), response);
            }
            if is_register {
                self.install_runtime(&key, self.host.clone(), None);
            }
            let (runtime, response) = self.finalize_registration(
                self.host.clone(),
                &context,
                register_worker_id.as_deref(),
                rollback_state.0,
                rollback_state.1,
                rollback_state.2,
                rollback_state.3,
                response,
            );
            return (runtime, self.retire_superseded_claimants(superseded, response));
        }

        let Req::Register { cwd, .. } = &req else {
            return (
                self.host.clone(),
                Resp::err(format!(
                    "PROJECT_SCOPE_UNKNOWN: no host route is registered for {}",
                    context.canonical_root
                )),
            );
        };
        if let Err(error) = validate_project_registration_cwd(cwd, &context_root) {
            return (self.host.clone(), Resp::err(error));
        }
        let storage_root = self.storage_root_for_new(
            &context_root,
            context.project_scope.as_str(),
            context.app_scope_id.as_str(),
        );
        // Admit the project route before opening or mutating its reducer. The
        // host journal is the durable transaction boundary: if it cannot be
        // published, this request must not create or mutate a project
        // journal that would be unreachable after a restart.
        if let Err(error) = self.append_route_record(&context, &storage_root) {
            return (self.host.clone(), Resp::err(error));
        }
        self.install_pending_route(&key, &context_root, &storage_root);
        let runtime = match self.ensure_runtime(&key, &context_root, &storage_root) {
            Ok(runtime) => runtime,
            Err(error) => return (self.host.clone(), Resp::err(error)),
        };
        let rollback_state = match register_worker_id.as_deref() {
            Some(worker_id) => {
                match self.registration_rollback_state(&runtime, &context, worker_id) {
                    Ok(state) => state,
                    Err(error) => return (runtime, Resp::err(error)),
                }
            }
            None => (None, None, None, Vec::new()),
        };
        let response = if let Err(error) = validate_request_context(&runtime, &req, Some(&context))
        {
            Resp::err(error)
        } else {
            dispatch_with_route_context(&runtime, req, Some(context.clone()))
        };
        let (runtime, response) = self.finalize_registration(
            runtime,
            &context,
            register_worker_id.as_deref(),
            rollback_state.0,
            rollback_state.1,
            rollback_state.2,
            rollback_state.3,
            response,
        );
        (runtime, self.retire_superseded_claimants(superseded, response))
    }
}
