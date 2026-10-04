impl ProjectRuntimeManager {
    fn new(host: Arc<Server>, host_paths: &crate::scope::HostPaths) -> Result<Arc<Self>, String> {
        let host_root = GlobalState::canonical_project_scope(&host.root)
            .map_err(|error| format!("PROJECT_SCOPE_UNKNOWN: {error}"))?;
        let route_journal = host_paths.state_root().join("routes.jsonl");
        let mut routes = std::collections::BTreeMap::new();

        // Preserve the resident route discovered from its own reducer.  Old
        // external registrations are retained as not-ready route metadata so
        // a client receives an explicit migration error until its project
        // runtime can be opened.
        let registry = HostRouteRegistry::for_server(&host)?;
        for ((app_scope, project_scope), owner) in registry.routes {
            match owner {
                HostRouteOwner::ResidentProject { root, .. } => {
                    routes.insert(
                        (app_scope, project_scope),
                        RuntimeRoute {
                            root: root.clone(),
                            storage_root: root,
                            runtime: Some(host.clone()),
                        },
                    );
                }
                HostRouteOwner::RegisteredNotReady { root } => {
                    routes
                        .entry((app_scope, project_scope))
                        .or_insert(RuntimeRoute {
                            root: root.clone(),
                            storage_root: root,
                            runtime: None,
                        });
                }
            }
        }

        let mut route_records = Vec::new();
        for record in load_host_route_records(&route_journal)? {
            if route_record_is_replayable(&record)? {
                route_records.push(record);
            }
        }
        Self::restore_resident_route_record(
            &host,
            host_root.as_str(),
            &route_journal,
            &route_records,
        )?;
        route_records.clear();
        for record in load_host_route_records(&route_journal)? {
            if route_record_is_replayable(&record)? {
                route_records.push(record);
            }
        }
        // A route record is only usable when its storage path has one owner.
        // The host resident reducer is an owner even when it has no entry in
        // routes.jsonl; otherwise replay could admit a second reducer on the
        // resident journal.
        for record in &route_records {
            let key = (record.app_scope_id.clone(), record.project_scope.clone());
            let storage_root = PathBuf::from(&record.storage_root);
            let canonical_root = Path::new(&record.canonical_root);
            let resident_self_route = is_resident_self_route(
                &record.app_scope_id,
                &record.project_scope,
                canonical_root,
                &storage_root,
                Path::new(host_root.as_str()),
                &host.storage_root,
                &host,
                &routes,
            )
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?;
            let resident_key = record.app_scope_id == crate::identity::CLI_APP_SERVER_ID
                && canonical_root == Path::new(host_root.as_str())
                && record.project_scope == host_root.as_str();
            if resident_key && !resident_self_route {
                return Err(format!(
                    "HOST_ROUTE_REPLAY_FAILED: resident route storage root {} does not match resident host",
                    record.storage_root
                ));
            }
            let mut owner_key = None;
            for (candidate_key, route) in &routes {
                if candidate_key != &key
                    && storage_roots_equal(&route.storage_root, &storage_root)
                        .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?
                {
                    owner_key = Some(candidate_key.clone());
                    break;
                }
            }
            if let Some(owner_key) = owner_key {
                return Err(format!(
                    "HOST_ROUTE_REPLAY_FAILED: runtime storage root {} is already owned by route ({}, {})",
                    record.storage_root, owner_key.0, owner_key.1
                ));
            }
        }

        for record in route_records {
            let (key, root, storage_root) = validate_host_route_record(&record)?;
            let resident_self_route = is_resident_self_route(
                &key.0,
                &key.1,
                &root,
                &storage_root,
                Path::new(host_root.as_str()),
                &host.storage_root,
                &host,
                &routes,
            )
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?;
            if resident_self_route {
                let registration = ProjectRegistration::with_registered_at(
                    ProjectScopeId::new(key.1.clone())
                        .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?,
                    AppServerId::new(key.0.clone())
                        .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?,
                    record.registered_ms,
                )
                .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?;
                if host
                    .state
                    .lock()
                    .unwrap()
                    .global
                    .lookup_registration(&registration.project_scope, &registration.app_scope_id)
                    .is_none()
                {
                    host.commit_checked(&[Event::GlobalProjectRegistered { registration }])
                        .map_err(|error| {
                            format!(
                                "HOST_ROUTE_REPLAY_FAILED: restore resident project registration: {error}"
                            )
                        })?;
                }
                routes.insert(
                    key,
                    RuntimeRoute {
                        root,
                        storage_root,
                        runtime: Some(host.clone()),
                    },
                );
                continue;
            }
            routes.insert(
                key,
                RuntimeRoute {
                    root,
                    storage_root,
                    runtime: None,
                },
            );
        }

        // Legacy registrations may be present in GlobalState while the host
        // route journal is empty. Validate the merged table before replay can
        // open any pending runtime; a pending legacy alias of the resident
        // storage must fail closed instead of creating a second reducer.
        validate_route_owner_table(&host, &routes)?;

        let manager = Arc::new(Self {
            host,
            host_root: PathBuf::from(host_root.as_str()),
            route_journal,
            routes: Mutex::new(routes),
            project_locks: Mutex::new(std::collections::BTreeMap::new()),
            register_gate: Mutex::new(()),
            runtime_init_gates: Mutex::new(std::collections::BTreeMap::new()),
            #[cfg(test)]
            fail_current_thread_route_publish: std::sync::atomic::AtomicBool::new(false),
        });

        // Replay every durable route at startup.  A broken external runtime
        // remains in the route table and is reported as NOT_READY on use;
        // it must never silently fall back to the resident reducer.
        let pending = manager
            .routes
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, route)| route.runtime.is_none())
            .map(|(key, route)| (key.clone(), route.root.clone(), route.storage_root.clone()))
            .collect::<Vec<_>>();
        for (key, root, storage_root) in pending {
            let _ = manager.ensure_runtime(&key, &root, &storage_root);
        }
        manager.reconcile_started_thread_routes()?;
        manager.reconcile_same_pane_master_routes()?;
        Ok(manager)
    }

    fn pending_same_pane_master_bindings(&self, runtime: &Server) -> Vec<RuntimeBinding> {
        let state = runtime.state.lock().unwrap();
        state
            .global
            .projects
            .values()
            .flat_map(|project| project.runtime_bindings.values())
            .filter(|binding| {
                let Some(endpoint) = binding.tmux_endpoint.as_ref() else {
                    return false;
                };
                let binding_text = binding.binding_id.as_str();
                let command_prefix = format!(
                    "register-{binding_text}-{}",
                    binding.endpoint_generation
                );
                let operation_prefix = format!(
                    "register-op-{binding_text}-{}",
                    binding.endpoint_generation
                );
                let recorded_operation = state.global.command_receipts.values().any(|receipt| {
                    let command = receipt.command_id.as_str();
                    let operation = receipt.operation_id.as_str();
                    let suffix = command.strip_prefix(&command_prefix);
                    suffix.is_some_and(|suffix| {
                        (suffix.is_empty() || suffix.starts_with("-retry-"))
                            && operation.strip_prefix(&operation_prefix) == Some(suffix)
                    })
                });
                recorded_operation
                    &&
                state
                    .global
                    .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
                    .is_some_and(|grant| grant.endpoint_generation == binding.endpoint_generation)
                    && state.workers.get(binding.agent_id.as_str()).is_some_and(|worker| {
                        selected_transport_for_worker(worker)
                            .is_some_and(|transport| transport.kind == TransportKind::Tmux)
                    })
            })
            .cloned()
            .collect()
    }

    fn same_pane_master_route_ready(&self, runtime: &Arc<Server>) -> Result<(), String> {
        if Arc::ptr_eq(runtime, &self.host) {
            return Ok(());
        }
        let pending = self.pending_same_pane_master_bindings(runtime);
        if pending.is_empty() {
            return Ok(());
        }
        for binding in pending {
            // A pane is addressable by exactly one live peer per project. Once
            // another binding in the same route scope owns this pane with a
            // live native thread, the stale same-pane master anchor is
            // superseded and must not fence the whole project route forever.
            if same_scope_pane_owner_supersedes(&self.host, runtime, &binding) {
                continue;
            }
            let route = {
                let host = self.host.state.lock().unwrap();
                binding
                    .tmux_endpoint
                    .as_ref()
                    .and_then(|endpoint| host.global.lookup_unique_tmux_pane_route(endpoint))
                    .cloned()
            };
            if route.as_ref() != Some(&binding) {
                return Err(format!(
                    "RECOVERY_RECONCILE_REQUIRED: host route for {} is not at project generation {}",
                    binding.agent_id, binding.endpoint_generation
                ));
            }
        }
        Ok(())
    }

    fn reconcile_same_pane_master_routes(&self) -> Result<(), String> {
        for runtime in self.runtimes() {
            if Arc::ptr_eq(&runtime, &self.host) {
                continue;
            }
            for binding in self.pending_same_pane_master_bindings(&runtime) {
                let Some(endpoint) = binding.tmux_endpoint.as_ref() else {
                    continue;
                };
                if same_scope_pane_owner_supersedes(&self.host, &runtime, &binding) {
                    continue;
                }
                let host_route = {
                    let host = self.host.state.lock().unwrap();
                    host.global.lookup_unique_tmux_pane_route(endpoint).cloned()
                };
                if host_route.as_ref() == Some(&binding) {
                    continue;
                }
                let Some(old) = host_route else {
                    self.host
                        .commit_checked(&[Event::GlobalCurrentThreadRouteSet { binding }])
                        .map_err(|error| {
                            format!("RECOVERY_RECONCILE_REQUIRED: {error}")
                        })?;
                    continue;
                };
                let is_same_durable_route = old.same_principal(&binding)
                    && old.binding_id == binding.binding_id
                    && old.runtime_id == binding.runtime_id
                    && old.session_id == binding.session_id
                    && old.native_thread_id == binding.native_thread_id
                    && old.tmux_endpoint.as_ref().is_some_and(|previous| {
                        crate::client::adapters::tmux::same_pane_route(previous, endpoint)
                    });
                if !is_same_durable_route {
                    return Err(format!(
                        "RECOVERY_RECONCILE_REQUIRED: host and project pane routes disagree for {}",
                        binding.agent_id
                    ));
                }
                if old.endpoint_generation >= binding.endpoint_generation {
                    return Err(format!(
                        "RECOVERY_RECONCILE_REQUIRED: host route for {} is at generation {} and project route is at generation {}",
                        binding.binding_id, old.endpoint_generation, binding.endpoint_generation
                    ));
                }
                self.host
                    .commit_checked(&[Event::GlobalCurrentThreadRouteSet { binding }])
                    .map_err(|error| format!("RECOVERY_RECONCILE_REQUIRED: {error}"))?;
            }
        }
        Ok(())
    }

    /// Replay-only publication of the authoritative project route.
    ///
    /// The project journal owns the latest runtime binding. The host journal
    /// owns the addressable current-thread index. When those indices disagree
    /// only because the host index was lagged, startup converges the host index
    /// to the project owner instead of waiting for a later send to discover a
    /// stale endpoint.
    fn reconcile_started_thread_routes(&self) -> Result<(), String> {
        let runtimes = self.runtimes();
        let bindings = runtimes.into_iter().flat_map(|runtime| {
            let bindings = runtime
                .state
                .lock()
                .unwrap()
                .global
                .projects
                .values()
                .flat_map(|project| project.runtime_bindings.values().cloned())
                .collect::<Vec<_>>();
            bindings.into_iter()
        });
        for binding in bindings {
            if binding.session_id.is_none() || binding.native_thread_id.is_none() {
                continue;
            }
            let session_id = binding.session_id.clone().unwrap();
            let native_thread_id = binding.native_thread_id.clone().unwrap();
            let current = self
                .host
                .state
                .lock()
                .unwrap()
                .global
                .lookup_current_thread_route(&session_id, &native_thread_id)
                .cloned();
            if current.as_ref() == Some(&binding) {
                continue;
            }
            let Some(old) = current else {
                self.publish_current_thread_route(&binding)
                    .map_err(|error| format!("RECOVERY_RECONCILE_REQUIRED: {error}"))?;
                continue;
            };
            let same_durable_route = old.same_principal(&binding)
                && old.binding_id == binding.binding_id
                && old.runtime_id == binding.runtime_id
                && old.project_scope == binding.project_scope
                && old.app_scope_id == binding.app_scope_id
                && old.session_id == binding.session_id
                && old.native_thread_id == binding.native_thread_id;
            if !same_durable_route {
                return Err(format!(
                    "RECOVERY_RECONCILE_REQUIRED: current thread route for {} belongs to {}",
                    native_thread_id, old.binding_id
                ));
            }
            if old.endpoint_generation >= binding.endpoint_generation {
                return Err(format!(
                    "RECOVERY_RECONCILE_REQUIRED: host route for {} is at generation {} and project route is at generation {}",
                    binding.binding_id, old.endpoint_generation, binding.endpoint_generation
                ));
            }
            self.publish_current_thread_route(&binding)
                .map_err(|error| format!("RECOVERY_RECONCILE_REQUIRED: {error}"))?;
        }
        Ok(())
    }

    fn publish_current_thread_route(&self, binding: &RuntimeBinding) -> Result<(), String> {
        self.host
            .commit_checked(&[Event::GlobalCurrentThreadRouteSet {
                binding: binding.clone(),
            }])
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn restore_resident_route_record(
        host: &Arc<Server>,
        host_root: &str,
        route_journal: &Path,
        route_records: &[HostRouteRecord],
    ) -> Result<(), String> {
        let project_scope = ProjectScopeId::new(host_root.to_owned())
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?;
        let app_scope = AppServerId::new(crate::identity::CLI_APP_SERVER_ID)
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?;
        let registration = host
            .state
            .lock()
            .unwrap()
            .global
            .lookup_registration(&project_scope, &app_scope)
            .cloned();
        let Some(registration) = registration else {
            return Ok(());
        };
        if route_records.iter().any(|record| {
            record.app_scope_id == registration.app_scope_id.as_str()
                && record.project_scope == registration.project_scope.as_str()
        }) {
            return Ok(());
        }

        let storage_root = storage_owner_path(&host.storage_root)
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?;
        let record = HostRouteRecord {
            version: 1,
            op: "register".into(),
            app_scope_id: registration.app_scope_id.as_str().into(),
            project_scope: registration.project_scope.as_str().into(),
            canonical_root: host_root.to_owned(),
            storage_root: storage_root.to_string_lossy().into_owned(),
            registered_ms: registration.registered_at_ms,
        };
        append_host_route_record(route_journal, &record)
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: restore resident route: {error}"))
    }

    fn validate_current_thread_candidate(
        &self,
        context: &ProjectContext,
        req: &Req,
    ) -> Result<(), String> {
        let Req::Register {
            worker_id,
            candidates: Some(candidates),
            retire_cross_project_anchor,
            ..
        } = req
        else {
            return Ok(());
        };
        let Some(candidate) = candidates.tmux.as_ref() else {
            return Ok(());
        };
        let endpoint = &candidate.endpoint;
        let mut runtimes = vec![self.host.clone()];
        for runtime in self.runtimes() {
            if !runtimes
                .iter()
                .any(|existing| Arc::ptr_eq(existing, &runtime))
            {
                runtimes.push(runtime);
            }
        }
        let mut anchor_matches = Vec::new();
        for runtime in runtimes {
            let state = runtime.state.lock().unwrap();
            for binding in state
                .global
                .projects
                .values()
                .flat_map(|project| project.runtime_bindings.values())
            {
                let same_codex_session =
                    endpoint.codex_session_id.as_deref().is_some_and(|session| {
                        binding
                            .session_id
                            .as_ref()
                            .map(crate::identity::SessionId::as_str)
                            == Some(session)
                    });
                let same_codex_thread = endpoint.codex_thread_id.as_deref().is_some_and(|thread| {
                    binding
                        .native_thread_id
                        .as_ref()
                        .map(NativeThreadId::as_str)
                        == Some(thread)
                });
                let same_tmux_pane = binding.tmux_endpoint.as_ref().is_some_and(|previous| {
                    // A live native thread is authoritative. The tmux pane is
                    // only a recovery anchor when both Codex IDs are absent,
                    // so a shared pane cannot block a second App Server peer.
                    // Pane identity has one owner (`same_pane_route`): socket,
                    // server pid, session, pane id and pane pid.
                    endpoint.codex_session_id.is_none()
                        && endpoint.codex_thread_id.is_none()
                        && crate::client::adapters::tmux::same_pane_route(previous, endpoint)
                });
                if (same_codex_session || same_codex_thread || same_tmux_pane)
                    && !anchor_matches.iter().any(|existing| existing == binding)
                {
                    anchor_matches.push(binding.clone());
                }
            }
        }
        if anchor_matches.len() > 1 {
            return Err(
                "RUNTIME_BINDING_REJECTED: tmux identity anchor matches multiple persisted peers"
                    .to_owned(),
            );
        }
        if let Some(binding) = anchor_matches.first() {
            if binding.agent_id.as_str() != worker_id {
                if !*retire_cross_project_anchor {
                    return Err(format!(
                        "RUNTIME_BINDING_REJECTED: tmux identity anchor is already bound to worker {}",
                        binding.agent_id
                    ));
                }
                self.retire_cross_project_anchor_candidate(context, binding)?;
            }
            let remaining_scope_mismatch = binding.app_scope_id != context.app_scope_id
                || binding.project_scope != context.project_scope;
            if remaining_scope_mismatch && !*retire_cross_project_anchor {
                return Err(
                    "RUNTIME_BINDING_REJECTED: tmux identity anchor belongs to another project route"
                        .to_owned(),
                );
            }
            if remaining_scope_mismatch {
                self.retire_cross_project_anchor_candidate(context, binding)?;
            }
        }
        Ok(())
    }

    fn retire_cross_project_anchor_candidate(
        &self,
        context: &ProjectContext,
        binding: &RuntimeBinding,
    ) -> Result<(), String> {
        let candidate_scope_mismatch = binding.app_scope_id != context.app_scope_id
            || binding.project_scope != context.project_scope;
        if !candidate_scope_mismatch {
            return Ok(());
        }
        // The reducer refuses to retire a route without both ids; report the
        // same precondition with a message that names the stale anchor.
        if binding.native_thread_id.is_none() {
            return Err(
                "RUNTIME_BINDING_REJECTED: stale cross-project anchor has no native thread id"
                    .to_owned(),
            );
        }
        if binding.session_id.is_none() {
            return Err(
                "RUNTIME_BINDING_REJECTED: stale cross-project anchor has no session id".to_owned(),
            );
        }
        // The anchor match that selected this binding already applied the single
        // pane-identity implementation (`same_pane_route`). Re-deriving pane
        // identity here a second time could only disagree with the match that got
        // us here, so the postcondition asks the exact question the reducer
        // answers: is this binding still a live current-thread route?
        self.host
            .commit_checked(&[Event::GlobalCurrentThreadRouteRetired {
                binding: binding.clone(),
            }])
            .map(|_| ())
            .map_err(|error| format!("RUNTIME_BINDING_REJECTED: {error}"))?;
        let retired = {
            let state = self.host.state.lock().unwrap();
            !state
                .global
                .current_thread_routes
                .values()
                .any(|route| route == binding)
        };
        if retired {
            Ok(())
        } else {
            Err(
                "RUNTIME_BINDING_REJECTED: stale cross-project anchor retirement was not applied"
                    .to_owned(),
            )
        }
    }
}
