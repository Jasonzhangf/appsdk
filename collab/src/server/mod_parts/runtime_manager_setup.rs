#[derive(Debug)]
enum BindingPublicationError {
    AddressConflict(String),
    Persistence(String),
}

impl std::fmt::Display for BindingPublicationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddressConflict(message) | Self::Persistence(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl ProjectRuntimeManager {
    fn new(host: Arc<Server>, host_paths: &crate::scope::HostPaths) -> Result<Arc<Self>, String> {
        let host_root = GlobalState::canonical_project_scope(&host.root)
            .map_err(|error| format!("PROJECT_SCOPE_UNKNOWN: {error}"))?;
        let route_file = host_paths.state_root().join("routes.json");
        // Finish committed deletion before current project state can
        // materialize old state. A real deletion failure stays explicit.
        let mut pending: Vec<String> = host
            .state
            .lock()
            .unwrap()
            .pending_project_resets
            .keys()
            .cloned()
            .collect();
        let map = crate::server::binding_store::BindingControlStore::new(host_paths.state_root()).load()?;
        for scope in map.pending_resets {
            if !pending.contains(&scope) { pending.push(scope); }
        }
        for scope in pending {
            project_reset::finish_reset(&host, &route_file, &scope)
                .map_err(|e| format!("RESET_INCOMPLETE: {e}"))?;
        }
        // Remove invalid records durably before the resident registry can
        // interpret them as recoverable registrations.
        for record in load_host_route_records(&route_file)? {
            if project_reset::route_is_deinitialized(Path::new(&record.canonical_root))? {
                project_reset::retire_stale_project_routes(&host, &record.project_scope)?;
                project_reset::delete_route(&route_file, &record)?;
            }
        }
        let mut routes = std::collections::BTreeMap::new();

        // Preserve the resident route discovered from its own reducer.  Old
        // external registrations are retained as not-ready route metadata so
        // a client receives an explicit migration error until its project
        // runtime can be opened.
        let registry = HostRouteRegistry::for_server(&host)?;
        for ((app_scope, project_scope), owner) in registry.routes {
            if project_reset::route_is_deinitialized(Path::new(&project_scope))? {
                project_reset::retire_stale_project_routes(&host, &project_scope)?;
                continue;
            }
            match owner {
                HostRouteOwner::ResidentProject { root, .. } => {
                    routes.insert(
                        (app_scope, project_scope),
                        RuntimeRoute {
                            root: root.clone(),
                            storage_root: host.storage_root.clone(),
                            runtime: Some(host.clone()),
                        },
                    );
                }
                HostRouteOwner::RegisteredNotReady { root } => {
                    let storage_root = host_paths.app_storage_root(&project_scope, &app_scope);
                    routes
                        .entry((app_scope, project_scope))
                        .or_insert(RuntimeRoute {
                            root: root.clone(),
                            storage_root,
                            runtime: None,
                        });
                }
            }
        }

        let mut route_records = Vec::new();
        for record in load_host_route_records(&route_file)? {
            if route_record_is_replayable(&record)? {
                route_records.push(record);
            }
        }
        if let Some(record) = Self::restore_resident_route_record(
            &host,
            host_root.as_str(),
            &route_file,
            &route_records,
        )? {
            if route_record_is_replayable(&record)? {
                route_records.push(record);
            }
        }
        // A route record is only usable when its storage path has one owner.
        // The host resident reducer is an owner even when it has no entry in
        // routes.json; another reducer cannot share the resident state owner.
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
            let (key, root, storage_root) = validate_host_route_record(&record, host_paths)?;
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
                let registration_missing = host
                    .state
                    .lock()
                    .unwrap()
                    .global
                    .lookup_registration(&registration.project_scope, &registration.app_scope_id)
                    .is_none();
                if registration_missing {
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

        // Validate the merged table before opening any pending runtime.
        // A pending alias of the resident
        // storage must fail closed instead of creating a second reducer.
        validate_route_owner_table(&host, &routes)?;
        // Namespace initialization is a write. Every pending route must first
        // pass the existing path, resident and shared-owner checks above.
        for (key, route) in &routes {
            if route.runtime.is_none() {
                select_central_runtime_storage(host_paths, &route_file, key, None)?;
            }
        }

        let manager = Arc::new(Self {
            host,
            host_root: PathBuf::from(host_root.as_str()),
            route_file,
            routes: Mutex::new(routes),
            project_locks: Mutex::new(std::collections::BTreeMap::new()),
            register_gate: Mutex::new(()),
            identity_gate: Mutex::new(()),
            federation_accept_gate: Mutex::new(()),
            runtime_init_gates: Mutex::new(std::collections::BTreeMap::new()),
            materialization_gate: Mutex::new(()),
            materialization_cap: std::sync::atomic::AtomicUsize::new(
                MAX_MATERIALIZED_EXTERNAL_RUNTIMES,
            ),
            #[cfg(test)]
            fail_current_thread_route_publish: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            materialization_build_delay_ms: std::sync::atomic::AtomicU64::new(0),
        });
        manager.prune_stale_routes()?;

        let binding_control =
            crate::server::binding_store::BindingControlStore::new(host_paths.state_root())
                .load()
                .map_err(|error| format!("BINDING_CONTROL_READ_FAILED: {error}"))?;
        let controlled_route_keys: std::collections::BTreeSet<RouteKey> = binding_control
            .active_values()
            .map(|value| {
                (
                    value.binding.app_scope_id.as_str().to_owned(),
                    value.binding.project_scope.as_str().to_owned(),
                )
            })
            .collect();

        // A durable route record is not itself an active runtime. Routes
        // addressed by the host index and routes owned by an active binding
        // control set are materialized at startup; every other initialized
        // route stays pending until the client selects it. A legacy route
        // file without a binding_control line therefore stays lazy: startup
        // must not materialize all initialized projects up front. Project
        // journals are never consulted to discover bindings.
        let controlled_route_count = binding_control.active_values().count();
        append_log(
            &host_paths.log_path(),
            &format!("BINDING_CONTROL_STARTUP_ROUTES: {controlled_route_count}"),
        );
        for value in binding_control.active_values() {
            let key = (
                value.binding.app_scope_id.as_str().to_owned(),
                value.binding.project_scope.as_str().to_owned(),
            );
            let storage_root = host_paths.app_storage_root(&key.1, &key.0);
            manager.install_pending_route(
                &key,
                Path::new(value.binding.project_scope.as_str()),
                &storage_root,
            );
        }
        let pending = manager
            .routes
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, route)| route.runtime.is_none())
            .map(|(key, route)| (key.clone(), route.root.clone(), route.storage_root.clone()))
            .collect::<Vec<_>>();
        for (key, root, storage_root) in pending {
            if !root.join(".agent-collab").is_dir() {
                append_log(
                    &host_paths.log_path(),
                    &format!(
                        "RUNTIME_ENSURE_SKIPPED_UNINITIALIZED: route {key:?} at {} (storage {}) is not materialized",
                        root.display(),
                        storage_root.display()
                    ),
                );
                continue;
            }
            if !controlled_route_keys.contains(&key) {
                append_log(
                    &host_paths.log_path(),
                    &format!(
                        "RUNTIME_MATERIALIZATION_DEFERRED: route {key:?} at {} (storage {}) has no active host route or control binding",
                        root.display(),
                        storage_root.display()
                    ),
                );
                continue;
            }
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
        manager.prune_stale_routes()?;
        // Consume the durable cross-file handoff before any route index is
        // republished. A pending effect is control truth from the interrupted
        // transaction; it must be applied or remain an explicit blocker before
        // the host starts addressing routes from the reducer.
        // A single unfinished business command stays blocked per scope through
        // the admission gate instead of stopping this daemon incarnation. Only
        // a corrupt control resource still fails the manager open.
        if let Err(error) = manager.recover_pending_effects() {
            append_log(
                &host_paths.log_path(),
                &format!("PENDING_EFFECT_RECOVERY_DEFERRED: {error}"),
            );
        }
        let binding_control = manager.binding_control_store().load()?;
        // Publish the map's exact current bindings. Business indexes never
        // supply an alternative identity or revive an unregistered slot.
        manager.reconcile_controlled_thread_routes(&binding_control)?;
        Ok(manager)
    }

    /// Republish one binding owned by the current recovery request.
    ///
    /// Recovery must not fail because another project has an unrelated stale
    /// route. The same pane and native-address conflicts still fail closed.
    fn reconcile_binding_publication(
        &self,
        binding: &RuntimeBinding,
    ) -> Result<(), BindingPublicationError> {
        self.prune_stale_routes()
            .map_err(BindingPublicationError::Persistence)?;
        if let Some(owner) = self.pane_owner_other_than(binding) {
            return Err(BindingPublicationError::AddressConflict(format!(
                "RECOVERY_RECONCILE_REQUIRED: pane for {} belongs to {} binding {}",
                binding.binding_id, owner.agent_id, owner.binding_id
            )));
        }

        let host = self.host.state.lock().unwrap();
        if let Some(endpoint) = binding.tmux_endpoint.as_ref() {
            if let Some(current) = host.global.lookup_unique_tmux_pane_route(endpoint) {
                if current == binding {
                    return Ok(());
                }
                let same_durable_route = current.same_principal(binding)
                    && current.binding_id == binding.binding_id
                    && current.runtime_id == binding.runtime_id
                    && current.project_scope == binding.project_scope
                    && current.app_scope_id == binding.app_scope_id;
                if !same_durable_route {
                    return Err(BindingPublicationError::AddressConflict(format!(
                        "RECOVERY_RECONCILE_REQUIRED: host pane route for {} belongs to {}",
                        binding.binding_id, current.binding_id
                    )));
                }
            }
        }
        if let (Some(session_id), Some(native_thread_id)) = (
            binding.session_id.as_ref(),
            binding.native_thread_id.as_ref(),
        ) {
            if let Some(current) = host
                .global
                .lookup_current_thread_route(session_id, native_thread_id)
            {
                if current == binding {
                    return Ok(());
                }
                let same_durable_route = current.same_principal(binding)
                    && current.binding_id == binding.binding_id
                    && current.runtime_id == binding.runtime_id
                    && current.project_scope == binding.project_scope
                    && current.app_scope_id == binding.app_scope_id
                    && current.session_id == binding.session_id
                    && current.native_thread_id == binding.native_thread_id;
                if !same_durable_route {
                    return Err(BindingPublicationError::AddressConflict(format!(
                        "RECOVERY_RECONCILE_REQUIRED: current thread route for {} belongs to {}",
                        native_thread_id, current.binding_id
                    )));
                }
            }
        }
        drop(host);
        self.host
            .commit_checked(&[Event::GlobalCurrentThreadRouteSet {
                binding: binding.clone(),
            }])
            .map_err(|error| {
                BindingPublicationError::Persistence(format!(
                    "RECOVERY_RECONCILE_REQUIRED: {error}"
                ))
            })?;
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
        route_file: &Path,
        route_records: &[HostRouteRecord],
    ) -> Result<Option<HostRouteRecord>, String> {
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
            return Ok(None);
        };
        if route_records.iter().any(|record| {
            record.app_scope_id == registration.app_scope_id.as_str()
                && record.project_scope == registration.project_scope.as_str()
        }) {
            return Ok(None);
        }

        let storage_root = storage_owner_path(&host.storage_root)
            .map_err(|error| format!("HOST_ROUTE_REPLAY_FAILED: {error}"))?;
        let record = HostRouteRecord {
            app_scope_id: registration.app_scope_id.as_str().into(),
            project_scope: registration.project_scope.as_str().into(),
            canonical_root: host_root.to_owned(),
            storage_root: storage_root.to_string_lossy().into_owned(),
            registered_ms: registration.registered_at_ms,
        };
        append_host_route_record(route_file, &record).map_err(|error| {
            format!("HOST_ROUTE_REPLAY_FAILED: restore resident route: {error}")
        })?;
        Ok(Some(record))
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
            }
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
        // each runtime owns its own current-state file. Such a claimant is only
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
        let _publication = self.register_gate.lock().unwrap();
        let control = match self.binding_control_store().load() {
            Ok(control) => control,
            Err(error) => return Resp::err(error),
        };
        for (taker, runtime, binding) in superseded {
            if control.holds_current_binding(&binding) {
                continue;
            }
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
                        "SUPERSEDED_CLAIMANT_RETIREMENT_FAILED: {error}; the current authorization supersedes the earlier claimant; daemon reconciliation owns cleanup"
                    ))
                }
            };
            if let Err(error) = runtime.commit_checked(&events) {
                return Resp::err(format!(
                    "SUPERSEDED_CLAIMANT_RETIREMENT_FAILED: {error}; the current authorization supersedes the earlier claimant; daemon reconciliation owns cleanup"
                ));
            }
        }
        response
    }
}
