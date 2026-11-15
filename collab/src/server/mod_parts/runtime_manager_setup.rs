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
    #[cfg(test)]
    fn new(host: Arc<Server>, host_paths: &crate::scope::HostPaths) -> Result<Arc<Self>, String> {
        Self::open(HostServices {
            config: host.config.clone(),
            host_paths: host_paths.clone(),
            legacy_state_path: host.state_path.clone(),
            register_gate: host.register_gate.clone(),
            appserver_candidate_check: host.appserver_candidate_check.clone(),
            appserver_notification_sink: host.appserver_notification_sink.clone(),
            appserver_thread_status: host.appserver_thread_status.clone(),
            appserver_thread_archive: host.appserver_thread_archive.clone(),
            runtime_handle: host.rex_monitor.runtime_handle(),
        })
    }

    fn open(host: HostServices) -> Result<Arc<Self>, String> {
        let host_control = host_control::HostControl::read_current(&host.host_paths)?;
        Self::with_control(host, host_control)
    }

    fn with_control(host: HostServices, host_control: host_control::HostControl) -> Result<Arc<Self>, String> {
        let host_paths = &host.host_paths;
        let route_file = host_paths.state_root().join("routes.json");
        // Finish committed deletion before current project state can
        // materialize old state. A real deletion failure stays explicit.
        let store = crate::server::binding_store::BindingControlStore::new(host_paths.state_root());
        host_control.handoff_pending_resets(&store)?;
        let pending = store.load()?.pending_resets;
        for scope in pending {
            if let Err(error) = project_reset::finish_reset(
                host_paths, &route_file, &scope,
                &[
                    host.legacy_state_path.clone(), host_paths.state_root().to_path_buf(),
                    host.legacy_state_path.with_file_name("daemon.lock"),
                ],
            ) {
                append_log(&host_paths.log_path(), &format!("RESET_INCOMPLETE: scope {scope}: {error}"));
            }
        }
        // Retire deinitialized fixed routes before any project can load.
        for record in load_host_route_records(&route_file)? {
            if project_reset::route_is_deinitialized(Path::new(&record.canonical_root))? {
                project_reset::retire_stale_project_routes(host_paths, &record.project_scope)?;
                project_reset::delete_route(&route_file, &record)?;
            }
        }
        let mut routes = std::collections::BTreeMap::new();

        // Startup reads only fixed routes, never project state to discover them.
        for record in load_host_route_records(&route_file)? {
            if !route_record_is_replayable(&record)? {
                continue;
            }
            let (key, root, storage_root) = validate_host_route_record(&record, host_paths)?;
            routes.insert(
                key,
                RuntimeRoute {
                    root,
                    storage_root,
                    runtime: None,
                },
            );
        }

        // Reject shared storage before opening any project runtime.
        validate_route_owner_table(host_paths, &routes)?;
        // Namespace initialization is a write. Every pending route must first
        // pass the existing path and shared-owner checks above.
        for (key, route) in &routes {
            if route.runtime.is_none() {
                select_central_runtime_storage(host_paths, &route_file, key, None)?;
            }
        }

        let manager = Arc::new(Self {
            host: Arc::new(host.clone()),
            host_control,
            route_file,
            routes: Mutex::new(routes),
            project_locks: Mutex::new(std::collections::BTreeMap::new()),
            register_gate: Arc::clone(&host.register_gate),
            identity_gate: Mutex::new(()),
            federation_accept_gate: Mutex::new(()),
            runtime_init_gates: Mutex::new(std::collections::BTreeMap::new()),
            materialization_gate: Mutex::new(()),
            materialization_cap: std::sync::atomic::AtomicUsize::new(
                MAX_MATERIALIZED_EXTERNAL_RUNTIMES,
            ),
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

    /// Validate the recovery request's binding against the current map.
    /// No secondary host address index is published.
    fn reconcile_binding_publication(
        &self,
        binding: &RuntimeBinding,
    ) -> Result<(), BindingPublicationError> {
        self.prune_stale_routes()
            .map_err(BindingPublicationError::Persistence)?;
        if let Some(owner) = self.pane_owner_other_than(binding)
            .map_err(BindingPublicationError::Persistence)? {
            return Err(BindingPublicationError::AddressConflict(format!(
                "RECOVERY_RECONCILE_REQUIRED: pane for {} belongs to {} binding {}",
                binding.binding_id, owner.agent_id, owner.binding_id
            )));
        }

        let map = self.binding_control_store().load()
            .map_err(BindingPublicationError::Persistence)?;
        if !map.holds_current_binding(binding) {
            return Err(BindingPublicationError::AddressConflict(
                "RECOVERY_RECONCILE_REQUIRED: binding is not current in identity-map".into()));
        }
        if let (Some(session), Some(thread)) = (&binding.session_id, &binding.native_thread_id) {
            if map.active_values().any(|value| value.binding != *binding
                && value.binding.session_id.as_ref() == Some(session)
                && value.binding.native_thread_id.as_ref() == Some(thread)) {
                return Err(BindingPublicationError::AddressConflict(
                    "RECOVERY_RECONCILE_REQUIRED: multiple current bindings own the session/thread".into()));
            }
        }
        Ok(())
    }

    /// An exact different current mapping that claims the same physical pane.
    fn pane_owner_other_than(&self, binding: &RuntimeBinding) -> Result<Option<RuntimeBinding>, String> {
        let map = self.binding_control_store().load()?;
        let owner = map.active_values().map(|value| &value.binding).find(|other| {
            *other != binding && binding.tmux_endpoint.as_ref().is_some_and(|endpoint| {
                other.tmux_endpoint.as_ref().is_some_and(|candidate| {
                    crate::client::adapters::tmux::same_owned_pane(endpoint, candidate)
                })
            })
        }).cloned();
        Ok(owner)
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
        let map = self.binding_control_store().load()?;
        let mut pane_claims: Vec<(Arc<Server>, RuntimeBinding)> = Vec::new();
        for binding in map.active_values().map(|value| &value.binding) {
            if binding.tmux_endpoint.as_ref().is_some_and(|previous| {
                crate::client::adapters::tmux::same_owned_pane(previous, endpoint)
            }) {
                let runtime = self.runtime_for_route_scope(&binding.route_scope())?;
                pane_claims.push((runtime, binding.clone()));
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
