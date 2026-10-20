impl ProjectRuntimeManager {
    fn prune_before_materialization(&self, key: &RouteKey, root: &Path) -> Result<(), String> {
        if !project_reset::route_is_deinitialized(root)? {
            return Ok(());
        }
        let _publication = self.register_gate.lock().unwrap();
        project_reset::retire_stale_project_routes(&self.host, &key.1)?;
        for record in load_host_route_records(&self.route_journal)? {
            if record.project_scope == key.1 {
                project_reset::delete_route(&self.route_journal, &record)?;
            }
        }
        self.routes
            .lock()
            .unwrap()
            .retain(|(_, project), _| project != &key.1);
        self.project_locks.lock().unwrap().remove(root);
        Err("PROJECT_SCOPE_UNKNOWN: deinitialized project routes were retired".into())
    }
    /// Remove positive invalidations only. Transport reachability is never
    /// consulted; idle and unreachable live identities remain routable.
    fn prune_stale_routes(&self) -> Result<(), String> {
        let _publication = self.register_gate.lock().unwrap();
        let records = load_host_route_records(&self.route_journal)?;
        for record in records {
            if project_reset::route_is_deinitialized(Path::new(&record.canonical_root))? {
                project_reset::retire_stale_project_routes(&self.host, &record.project_scope)?;
                project_reset::delete_route(&self.route_journal, &record)?;
                self.routes
                    .lock()
                    .unwrap()
                    .remove(&(record.app_scope_id, record.project_scope.clone()));
                self.project_locks
                    .lock()
                    .unwrap()
                    .remove(Path::new(&record.project_scope));
            }
        }
        // Reconcile indexed addresses against the binding owner's committed
        // generation. A pending/unmaterialized runtime is unknown, not stale.
        let indexed: Vec<RuntimeBinding> = {
            let state = self.host.state.lock().unwrap();
            state
                .global
                .current_thread_routes
                .values()
                .chain(state.global.legacy_thread_routes.values())
                .cloned()
                .collect()
        };
        for binding in indexed {
            let runtime = self
                .routes
                .lock()
                .unwrap()
                .get(&(
                    binding.app_scope_id.as_str().to_owned(),
                    binding.project_scope.as_str().to_owned(),
                ))
                .and_then(|route| route.runtime.clone());
            let Some(runtime) = runtime else {
                continue;
            };
            let invalid = {
                let state = runtime.state.lock().unwrap();
                state
                    .global
                    .lookup_binding_for(&binding.route_scope(), &binding.binding_id)
                    .is_none_or(|current| current != &binding)
            };
            if invalid {
                self.host
                    .commit_checked(&[Event::GlobalCurrentThreadRouteRetired { binding }])
                    .map_err(|e| format!("STALE_ROUTE_CLEANUP_FAILED: {e}"))?;
            }
        }
        Ok(())
    }

    fn reset_project(&self, context: &ProjectContext, worker_id: &str, token: &str) -> Resp {
        // Lock order matches registration. The wire gate also excludes task,
        // mailbox and identity mutations for this root during the destructive phase.
        let runtime = match self.select_runtime(context) {
            Ok(runtime) => runtime,
            Err(error) => return Resp::err(error),
        };
        let _identity = self.identity_gate.lock().unwrap();
        // Resident route establishment takes materialization before route
        // publication. Reset uses that same global order across projects.
        let _materialization = self.materialization_gate.lock().unwrap();
        let _register = self.register_gate.lock().unwrap();
        let gate = wire_route_mutation_gate(&runtime);
        let _wire = gate.lock().unwrap();
        {
            let state = runtime.state.lock().unwrap();
            if let Err(response) = verify(&state, worker_id, token) {
                return response;
            }
            let route_scope = RouteScope {
                project_scope_id: context.project_scope.clone(),
                app_scope_id: context.app_scope_id.clone(),
            };
            if current_master_worker_id(&state, Some(&route_scope)).as_deref() != Some(worker_id) {
                return Resp::err("RESET_MASTER_REQUIRED: current project master is required");
            }
        }
        let scope = context.project_scope.as_str();
        // Capture exact local ownership before any reducer is cleared. Keep
        // these identifiers in the reset event so replay never needs cwd I/O.
        let worker_ids: Vec<String> = runtime
            .state
            .lock()
            .unwrap()
            .workers
            .values()
            .filter(|worker| worker.cwd == runtime.root.to_string_lossy() || worker.cwd == scope)
            .map(|worker| worker.id.clone())
            .collect();
        let clear_host_local = self.host_root == Path::new(scope);
        let host_worker_ids: Vec<String> = self
            .host
            .state
            .lock()
            .unwrap()
            .workers
            .values()
            .filter(|worker| worker.cwd == runtime.root.to_string_lossy() || worker.cwd == scope)
            .map(|worker| worker.id.clone())
            .collect();
        if let Err(error) = self.host.commit_checked(&[Event::ProjectResetStarted {
            project_scope: scope.to_owned(),
            clear_local: clear_host_local,
            worker_ids: host_worker_ids,
        }]) {
            return Resp::err(format!("RESET_INCOMPLETE: persist reset intent: {error}"));
        }
        let result = (|| -> Result<(), String> {
            self.operation_journal.reset_project(scope)?;
            // Remove all app scopes, including already materialized reducers.
            let runtimes: Vec<Arc<Server>> = self
                .routes
                .lock()
                .unwrap()
                .iter()
                .filter(|((_, project), _)| project == scope)
                .filter_map(|(_, route)| route.runtime.clone())
                .collect();
            for project in runtimes {
                if !Arc::ptr_eq(&project, &self.host) {
                    project
                        .commit_checked(&[Event::ProjectResetStarted {
                            project_scope: scope.to_owned(),
                            clear_local: true,
                            worker_ids: worker_ids.clone(),
                        }])
                        .map_err(|e| format!("clear project reducer: {e}"))?;
                }
            }
            self.routes
                .lock()
                .unwrap()
                .retain(|(_, project), _| project != scope);
            self.project_locks.lock().unwrap().remove(Path::new(scope));
            self.runtime_init_gates
                .lock()
                .unwrap()
                .retain(|(_, project), _| project != scope);
            project_reset::finish_reset(&self.host, &self.route_journal, scope)
        })();
        match result {
            Ok(()) => Resp::data(json!({"status": "complete", "project_scope": scope,
                "daemon_pid": std::process::id()})),
            Err(error) => Resp::err(format!("RESET_INCOMPLETE: {error}")),
        }
    }
}
