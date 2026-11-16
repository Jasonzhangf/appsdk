include!("runtime_manager_setup.rs");

impl ProjectRuntimeManager {
    fn runtimes(&self) -> Vec<Arc<Server>> {
        let mut result = Vec::new();
        let routes = self.routes.lock().unwrap();
        for route in routes.values() {
            if let Some(runtime) = &route.runtime {
                // The scheduler owns a runtime, not a route key;
                // ticking once per key would duplicate wakeups and could
                // consume a timer twice in the same interval.
                if result.iter().any(|existing| Arc::ptr_eq(existing, runtime)) {
                    continue;
                }
                result.push(runtime.clone());
            }
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

    fn resolve_route_by_native_session(
        &self,
        context: &ProjectContext,
        session_id: &str,
    ) -> Result<RouteResolution, String> {
        context.validate_registered_root(Path::new(&context.canonical_root))
            .map_err(|error| format!("ROUTE_RESOLVE_INVALID: {error}"))?;
        let session = crate::identity::SessionId::new(session_id.to_owned())
            .map_err(|error| format!("ROUTE_RESOLVE_INVALID: {error}"))?;
        let binding = {
            let map = self.binding_control_store().load()?;
            let mut matches = map.active_values().map(|value| &value.binding).filter(|binding| {
                binding.project_scope == context.project_scope
                    && binding.app_scope_id == context.app_scope_id
                    && binding.session_id.as_ref() == Some(&session)
            });
            let binding = matches.next().cloned().ok_or_else(|| {
                format!("ROUTE_RESOLVE_NOT_FOUND: no current route for this project/app/session; run collab context with current runtime facts")
            })?;
            if matches.next().is_some() {
                return Err("ROUTE_RESOLVE_AMBIGUOUS: multiple current routes for this project/app/session; run collab context with current runtime facts".into());
            }
            binding
        };
        let (route, _) = self.current_binding_snapshot(&binding)?;
        route.validate().map_err(|error| format!("ROUTE_RESOLVE_INVALID: {error}"))?;
        Ok(route)
    }

    /// Pure shared readback. It never loads a runtime, verifies transport
    /// liveness, registers an identity or repairs a publication.
    fn current_binding_snapshot(
        &self,
        binding: &RuntimeBinding,
    ) -> Result<(RouteResolution, Arc<Server>), String> {
        let key = (
            binding.app_scope_id.as_str().to_owned(),
            binding.project_scope.as_str().to_owned(),
        );
        let (storage_root, runtime) = {
            let routes = self.routes.lock().unwrap();
            let route = routes.get(&key).ok_or_else(|| {
                "ROUTE_RESOLVE_STALE_INDEX: current binding references an unknown route".to_owned()
            })?;
            let runtime = route.runtime.clone().ok_or_else(|| {
                "ROUTE_RESOLVE_STALE_INDEX: current binding references an unavailable runtime".to_owned()
            })?;
            (route.storage_root.clone(), runtime)
        };
        let registered = runtime.state.lock().unwrap().global
            .lookup_binding_for(&binding.route_scope(), &binding.binding_id).cloned()
            .ok_or_else(|| "ROUTE_RESOLVE_STALE_INDEX: current route references a missing runtime binding".to_owned())?;
        if &registered != binding {
            return Err("ROUTE_RESOLVE_INVALID: current route conflicts with its runtime binding".into());
        }
        Ok((RouteResolution {
            app_scope_id: binding.app_scope_id.clone(),
            project_scope: binding.project_scope.clone(),
            canonical_root: binding.project_scope.as_str().to_owned(),
            storage_root: storage_root.to_string_lossy().into_owned(),
            agent_id: binding.agent_id.clone(),
            binding_id: binding.binding_id.clone(),
            session_id: binding.session_id.clone(),
            native_thread_id: binding.native_thread_id.clone(),
        }, runtime))
    }

    fn resolve_route_by_tmux_endpoint(
        &self,
        endpoint: &crate::proto::TmuxEndpoint,
    ) -> Result<RouteResolution, String> {
        crate::client::adapters::tmux::validate_endpoint(endpoint)?;
        self.resolve_route_by_address(&endpoint.tmux_session_id, &endpoint.pane_id, Some(endpoint))
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
        let map = self.binding_control_store().load()?;
        let mut matches = map.active_values().map(|value| &value.binding).filter(|binding| {
            if let Some(requested) = requested_tmux_endpoint {
                binding.tmux_endpoint.as_ref().is_some_and(|endpoint| {
                    crate::client::adapters::tmux::same_pane_route(requested, endpoint)
                })
            } else {
                binding.session_id.as_ref() == Some(&session_id)
                    && binding.native_thread_id.as_ref() == Some(&native_thread_id)
            }
        });
        let binding = matches.next().cloned().ok_or_else(|| format!(
            "ROUTE_RESOLVE_NOT_FOUND: no current identity-map binding for session {session_id} and thread {native_thread_id}; {ROUTE_RESOLVE_NOT_FOUND_RECOVERY}"
        ))?;
        if matches.next().is_some() {
            return Err("ROUTE_RESOLVE_AMBIGUOUS: multiple current identity-map bindings own this address; run collab context with current runtime facts".into());
        }
        let (mut route, runtime) = self.current_binding_snapshot(&binding)?;
        let session_id = match binding.session_id.clone() {
            Some(bound) => Some(bound),
            None if binding.tmux_endpoint.is_some() => None,
            None => {
                return Err(format!(
                    "ROUTE_RESOLVE_INVALID: current route state for App Server thread {native_thread_id} has no session id"
                ))
            }
        };
        let canonical_root = binding.project_scope.as_str().to_owned();
        let transport = {
            let state = runtime.state.lock().unwrap();
            state
                .workers
                .get(binding.agent_id.as_str())
                .and_then(selected_transport_for_worker)
                .ok_or_else(|| {
                    format!(
                        "ROUTE_RESOLVE_INVALID: current route state for App Server thread {native_thread_id} has no selected transport"
                    )
                })?
        };
        match transport.kind {
            TransportKind::Rex => {
                return Err("ROUTE_RESOLVE_INVALID: Rex routes use the agent session without a native thread address".into());
            }
            TransportKind::DesktopBridge => {
                if requested_tmux_endpoint.is_some() {
                    return Err("ROUTE_RESOLVE_NOT_FOUND: Desktop route has no tmux endpoint".into());
                }
                crate::client::adapters::desktop_bridge::read_thread_status(&transport, native_thread_id.as_str())?;
            }
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
                    session_id: session_id.as_ref().ok_or("ROUTE_RESOLVE_INVALID: App Server has no session")?.as_str().to_owned(),
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
        route.session_id = session_id;
        route
            .validate()
            .map_err(|error| format!("ROUTE_RESOLVE_INVALID: {error}"))?;
        Ok(route)
    }

    fn finalize_registration(
        &self,
        runtime: Arc<Server>,
        worker_id: Option<&str>,
        response: Resp,
        register_effect_id: Option<&str>,
    ) -> (Arc<Server>, Resp) {
        let Some(_worker_id) = worker_id.filter(|_| response.ok) else {
            return (runtime, response);
        };
        match self.consume_register_effect(register_effect_id) {
            Ok(()) => (runtime, response),
            Err(error) => (runtime, Resp::err_data(format!("CONTROL_COMMITTED_EFFECTS_PENDING: {error}"), response.data)),
        }
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

    fn storage_root_for_new(&self, _root: &Path, project_scope: &str, app_scope: &str) -> PathBuf {
        self.host
            .host_paths
            .app_storage_root(project_scope, app_scope)
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
        requested_root: &Path,
        _requested_storage_root: &Path,
    ) -> Result<Arc<Server>, String> {
        if self.binding_control_store().load()?.pending_resets.contains(&key.1)
        {
            return Err(
                "RESET_INCOMPLETE: project cleanup must complete before materialization".into(),
            );
        }
        self.prune_before_materialization(key, requested_root)?;
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
        let root = self
            .routes
            .lock()
            .unwrap()
            .get(key)
            .map(|route| route.root.clone())
            .unwrap_or_else(|| requested_root.to_path_buf());
        let storage_root = select_central_runtime_storage(
            &self.host.host_paths, &self.route_file, key, Some(&self.register_gate),
        )?;
        self.routes.lock().unwrap().entry(key.clone())
            .and_modify(|route| route.storage_root = storage_root.clone());
        // The per-route gate above deduplicates one key. Admission of the
        // external-runtime budget must also be one decision across different
        // keys: two concurrent materializations must not both observe the
        // same count below the cap and then install past it.
        let _admission = self.materialization_gate.lock().unwrap();
        let materialized_external = self.runtimes().len();
        let cap = self
            .materialization_cap
            .load(std::sync::atomic::Ordering::Relaxed);
        if let Some(error) = materialized_runtime_budget_error(materialized_external, cap) {
            return Err(error);
        }
        #[cfg(test)]
        {
            let delay = self
                .materialization_build_delay_ms
                .load(std::sync::atomic::Ordering::Relaxed);
            if delay > 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay));
            }
        }
        let (runtime, project_lock) = self.build_runtime(key, &root, &storage_root)?;
        self.install_runtime(key, runtime.clone(), project_lock);
        Ok(runtime)
    }
}
include!("part_04/runtime.rs");
include!("part_04/binding_control.rs");
