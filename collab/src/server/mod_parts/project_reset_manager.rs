impl ProjectRuntimeManager {
    fn prune_before_materialization(&self, key: &RouteKey, root: &Path) -> Result<(), String> {
        if !project_reset::route_is_deinitialized(root)? {
            return Ok(());
        }
        let _publication = self.register_gate.lock().unwrap();
        project_reset::retire_stale_project_routes(&self.host.host_paths, &key.1)?;
        for record in load_host_route_records(&self.route_file)? {
            if record.project_scope == key.1 {
                project_reset::delete_route(&self.route_file, &record)?;
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
        let records = load_host_route_records(&self.route_file)?;
        for record in records {
            if project_reset::route_is_deinitialized(Path::new(&record.canonical_root))? {
                project_reset::retire_stale_project_routes(&self.host.host_paths, &record.project_scope)?;
                project_reset::delete_route(&self.route_file, &record)?;
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
        // Route establishment takes materialization before route
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
        if let Err(error) = self.binding_control_store().begin_reset(scope) {
            return Resp::err(format!("RESET_INCOMPLETE: publish identity reset intent: {error}"));
        }
        let result = (|| -> Result<(), String> {
            // Remove all app scopes, including already materialized reducers.
            let mut runtimes: Vec<Arc<Server>> = Vec::new();
            for ((_, project), route) in self.routes.lock().unwrap().iter() {
                if project == scope {
                    if let Some(runtime) = &route.runtime {
                        if !runtimes.iter().any(|saved| Arc::ptr_eq(saved, runtime)) {
                            runtimes.push(runtime.clone());
                        }
                    }
                }
            }
            for project in runtimes {
                let worker_ids = project.state.lock().unwrap().workers.values()
                    .filter(|worker| worker.cwd == project.root.to_string_lossy() || worker.cwd == scope)
                    .map(|worker| worker.id.clone()).collect();
                project
                    .commit_checked(&[Event::ProjectResetStarted {
                        project_scope: scope.to_owned(),
                        clear_local: true,
                        worker_ids,
                    }])
                    .map_err(|e| format!("clear project reducer: {e}"))?;
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
            project_reset::finish_reset(
                &self.host.host_paths, &self.route_file, scope,
                &[
                    self.host.legacy_state_path.clone(),
                    self.host.host_paths.state_root().to_path_buf(),
                    self.host.legacy_state_path.with_file_name("daemon.lock"),
                ],
            )
        })();
        match result {
            Ok(()) => Resp::data(json!({"status": "complete", "project_scope": scope,
                "daemon_pid": std::process::id()})),
            Err(error) => Resp::err(format!("RESET_INCOMPLETE: {error}")),
        }
    }
}
