impl ProjectRuntimeManager {
    fn new(host: Arc<Server>, host_paths: &crate::scope::HostPaths) -> Result<Arc<Self>, String> {
        let operation_journal = Arc::new(
            crate::server::operation_journal::OperationJournal::open(host_paths.journal_path())
                .map_err(|error| format!("OPERATION_JOURNAL_REPLAY_FAILED: {error}"))?,
        );
        Self::new_with_operation_journal(host, host_paths, operation_journal)
    }

    fn new_with_operation_journal(
        host: Arc<Server>,
        host_paths: &crate::scope::HostPaths,
        operation_journal: Arc<crate::server::operation_journal::OperationJournal>,
    ) -> Result<Arc<Self>, String> {
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
            operation_journal,
            routes: Mutex::new(routes),
            project_locks: Mutex::new(std::collections::BTreeMap::new()),
            register_gate: Mutex::new(()),
            identity_gate: Mutex::new(()),
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
            // A route whose runtime cannot be opened must stay visibly not
            // ready.  Dropping this error would hide the only trace of the
            // failure until a client happens to ask for that project.
            if let Err(error) = manager.ensure_runtime(&key, &root, &storage_root) {
                append_log(
                    &host_paths.log_path(),
                    &format!(
                        "RUNTIME_ENSURE_FAILED: route {key:?} at {} (storage {}) is not ready: {error}",
                        root.display(),
                        storage_root.display()
                    ),
                );
            }
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
                if binding.tmux_endpoint.is_none() {
                    return false;
                }
                let binding_text = binding.binding_id.as_str();
                let command_prefix =
                    format!("register-{binding_text}-{}", binding.endpoint_generation);
                let operation_prefix =
                    format!("register-op-{binding_text}-{}", binding.endpoint_generation);
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
                    && state
                        .global
                        .lookup_master_grant_for(&binding.route_scope(), &binding.binding_id)
                        .is_some_and(|grant| {
                            grant.endpoint_generation == binding.endpoint_generation
                        })
                    && state
                        .workers
                        .get(binding.agent_id.as_str())
                        .is_some_and(|worker| {
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
            let Some(endpoint) = binding.tmux_endpoint.as_ref() else {
                continue;
            };
            let owner = self.pane_owner_other_than(&binding);
            let is_claimant = {
                let host = self.host.state.lock().unwrap();
                host.global.lookup_unique_tmux_pane_route(endpoint) == Some(&binding)
            };
            if let Some(owner) = owner {
                // One pane owns exactly one binding host-wide, so a live route
                // on this pane at another address is the owner. The stale
                // same-pane master anchor is superseded by it, and this is the
                // request path: fencing here would reproduce the original
                // opaque failure on every non-Register request.
                append_log(
                    &self.host.log_path(),
                    &format!(
                        "RECOVERY_RECONCILE_SKIPPED_SUPERSEDED: {} lost pane {}:{} to {} binding {}; request fencing skipped",
                        binding.binding_id,
                        endpoint.tmux_session_id,
                        endpoint.pane_id,
                        owner.agent_id,
                        owner.binding_id
                    ),
                );
                continue;
            }
            if !is_claimant {
                return Err(format!(
                    "RECOVERY_RECONCILE_REQUIRED: host route for {} is not at project generation {}; re-register the worker to publish the route",
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
                if let Some(owner) = self.pane_owner_other_than(&binding) {
                    // A republisher never evicts. One pane owns exactly one
                    // binding host-wide, and this pane belongs to another
                    // route address, so this pending master anchor stays out
                    // of the index. Releasing the pane needs either the owner's
                    // own route removal or a fresh registration, which is a
                    // writer path and does evict.
                    append_log(
                        &self.host.log_path(),
                        &format!(
                            "RECOVERY_RECONCILE_SKIPPED_SUPERSEDED: {} lost pane {}:{} to {} binding {}",
                            binding.binding_id,
                            endpoint.tmux_session_id,
                            endpoint.pane_id,
                            owner.agent_id,
                            owner.binding_id
                        ),
                    );
                    continue;
                }
                let host_route = {
                    let host = self.host.state.lock().unwrap();
                    host.global.lookup_unique_tmux_pane_route(endpoint).cloned()
                };
                if host_route.as_ref() == Some(&binding) {
                    continue;
                }
                // The guard above skipped every claimant at another address, so
                // the only remaining claimant is this binding's own route at
                // the same address and a lower generation. Publishing it is a
                // refresh of this address, never an eviction.
                if let Some(old) = host_route.as_ref() {
                    if old.endpoint_generation >= binding.endpoint_generation {
                        return Err(format!(
                            "RECOVERY_RECONCILE_REQUIRED: host route for {} is at generation {} and project route is at generation {}; re-register the worker to advance the route",
                            binding.binding_id, old.endpoint_generation, binding.endpoint_generation
                        ));
                    }
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
            // A republisher never evicts. One pane owns exactly one binding
            // host-wide, so when another route address holds this pane, this
            // project route stays out of the index until the owner releases it
            // or a fresh registration takes the pane.
            if let Some(owner) = self.pane_owner_other_than(&binding) {
                append_log(
                    &self.host.log_path(),
                    &format!(
                        "RECOVERY_RECONCILE_SKIPPED_SUPERSEDED: {} lost pane to {} binding {}",
                        binding.binding_id, owner.agent_id, owner.binding_id
                    ),
                );
                continue;
            }
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
                self.host
                    .commit_checked(&[Event::GlobalCurrentThreadRouteSet {
                        binding: binding.clone(),
                    }])
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
            self.host
                .commit_checked(&[Event::GlobalCurrentThreadRouteSet {
                    binding: binding.clone(),
                }])
                .map_err(|error| format!("RECOVERY_RECONCILE_REQUIRED: {error}"))?;
        }
        Ok(())
    }

    /// The live route that owns this binding's pane as a different claim.
    ///
    /// One pane owns exactly one binding host-wide, so this is the single
    /// question every republisher must ask before publishing. A claimant at
    /// this binding's own address is not an owner: a generation refresh keeps
    /// its address and must still be published. A claimant that is this same
    /// durable binding at an older address is this route's own stale entry, and
    /// the reconciler advances it instead of treating it as another owner. The
    /// caller decides what to log, because the request path and the startup
    /// paths report different things.
    fn pane_owner_other_than(&self, binding: &RuntimeBinding) -> Option<RuntimeBinding> {
        let host = self.host.state.lock().unwrap();
        host.global.pane_claimant_other_than(binding).cloned()
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
    ) -> Result<Vec<(String, Arc<Server>, RuntimeBinding)>, String> {
        let Req::Register {
            worker_id,
            candidates: Some(candidates),
            ..
        } = req
        else {
            return Ok(Vec::new());
        };
        let Some(candidate) = candidates.tmux.as_ref() else {
            return Ok(Vec::new());
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
        let mut pane_claims: Vec<(Arc<Server>, RuntimeBinding)> = Vec::new();
        let mut foreign_thread_matches = Vec::new();
        for runtime in runtimes {
            let state = runtime.state.lock().unwrap();
            for binding in state
                .global
                .projects
                .values()
                .flat_map(|project| project.runtime_bindings.values())
            {
                // A tmux registration anchors on its pane id and nothing else.
                // The Codex ids a tmux endpoint carries are not queryable, and
                // the pane's shell pid is not an identity, so neither may decide
                // which binding a second claim replaces. Socket, session and
                // pane id name one pane on one tmux server.
                let same_tmux_pane = binding.tmux_endpoint.as_ref().is_some_and(|previous| {
                    crate::client::adapters::tmux::same_owned_pane(previous, endpoint)
                });
                if same_tmux_pane {
                    if !pane_claims.iter().any(|(_, existing)| existing == binding) {
                        pane_claims.push((runtime.clone(), binding.clone()));
                    }
                    continue;
                }
                // The Codex ids are not an anchor, but they still name one live
                // thread. Another worker holding this thread on a *different*
                // pane is a conflict, not a takeover: only a pane changes owner
                // here, so this stays refused and nothing is committed.
                if binding.agent_id.as_str() != worker_id {
                    let same_codex_session =
                        endpoint.codex_session_id.as_deref().is_some_and(|session| {
                            binding
                                .session_id
                                .as_ref()
                                .map(crate::identity::SessionId::as_str)
                                == Some(session)
                        });
                    let same_codex_thread =
                        endpoint.codex_thread_id.as_deref().is_some_and(|thread| {
                            binding
                                .native_thread_id
                                .as_ref()
                                .map(NativeThreadId::as_str)
                                == Some(thread)
                        });
                    if (same_codex_session || same_codex_thread)
                        && !foreign_thread_matches
                            .iter()
                            .any(|existing| existing == binding)
                    {
                        foreign_thread_matches.push(binding.clone());
                    }
                }
            }
        }
        if let Some(conflict) = foreign_thread_matches.first() {
            let identity = conflict
                .native_thread_id
                .as_ref()
                .map(NativeThreadId::as_str)
                .or_else(|| {
                    conflict
                        .session_id
                        .as_ref()
                        .map(crate::identity::SessionId::as_str)
                })
                .unwrap_or("<unknown>");
            return Err(format!(
                "RUNTIME_BINDING_REJECTED: Codex thread {identity} is already bound to worker {} on another pane",
                conflict.agent_id
            ));
        }
        // The pane scan above only answers the conflict question. Ownership of
        // the pane itself is settled by the registration commit, because both
        // registration entry points share that commit and neither may keep a
        // second owner of one pane.
        let route_scope = RouteScope {
            app_scope_id: context.app_scope_id.clone(),
            project_scope_id: context.project_scope.clone(),
        };
        // A claimant that lives in the runtime this registration commits into is
        // retired by that commit itself, so the replacement is one transaction.
        // A claimant in another runtime cannot join that transaction, because
        // each runtime owns its own journal. Such a claimant is therefore only
        // *planned* here and retired after the registration is durable: retiring
        // it first would close it for a registration that may still fail, and
        // that would strand the pane.
        let registering_runtime = self
            .routes
            .lock()
            .unwrap()
            .get(&Self::route_key(context))
            .and_then(|route| route.runtime.clone());
        let mut superseded = Vec::new();
        for (runtime, binding) in pane_claims {
            if registering_runtime
                .as_ref()
                .is_some_and(|owner| Arc::ptr_eq(owner, &runtime))
            {
                continue;
            }
            // The binding this registration is about to own is not a takeover:
            // a same-scope re-registration stays idempotent.
            if binding.agent_id.as_str() == worker_id && binding.route_scope() == route_scope {
                continue;
            }
            superseded.push((worker_id.clone(), runtime, binding));
        }
        Ok(superseded)
    }

    /// Retires the claimants that `validate_current_thread_candidate` planned,
    /// but only after the registering runtime committed the takeover. The order
    /// is the point: a registration that fails leaves the incumbent owning the
    /// pane instead of stranding it, and a retirement that fails leaves the pane
    /// owned by the new registrant instead of unowned.
    fn retire_superseded_claimants(
        &self,
        superseded: Vec<(String, Arc<Server>, RuntimeBinding)>,
        response: Resp,
    ) -> Resp {
        if !response.ok || superseded.is_empty() {
            return response;
        }
        for (taker, runtime, binding) in superseded {
            // The registration commit of the registering runtime already retires
            // a claimant that lives in that same runtime. Re-read the claimant
            // before retiring it, so a binding that is already retired is left
            // alone and one takeover never retires the same anchor twice.
            let still_owns_the_pane = {
                let state = runtime.state.lock().unwrap();
                state
                    .global
                    .projects
                    .values()
                    .flat_map(|project| project.runtime_bindings.values())
                    .any(|current| {
                        current.binding_id == binding.binding_id
                            && current.endpoint_generation == binding.endpoint_generation
                            && current.tmux_endpoint.is_some()
                    })
            };
            if !still_owns_the_pane {
                continue;
            }
            // The claimant is retired on the runtime that holds it, because only
            // that runtime's global state knows the claimant's project scope.
            let events = match pane_reclaim_events(&taker, &binding) {
                Ok(events) => events,
                Err(error) => {
                    return Resp::err(format!(
                        "SUPERSEDED_CLAIMANT_RETIREMENT_FAILED: {error}; the pane now belongs to the later registration; restart the affected daemon so the lagged claimant is reconciled"
                    ))
                }
            };
            if let Err(error) = runtime.commit_checked(&events) {
                return Resp::err(format!(
                    "SUPERSEDED_CLAIMANT_RETIREMENT_FAILED: {error}; the pane now belongs to the later registration; restart the affected daemon so the lagged claimant is reconciled"
                ));
            }
        }
        response
    }
}
