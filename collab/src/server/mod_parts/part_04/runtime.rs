impl ProjectRuntimeManager {
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
            let master_holder = match current_master_holder(&runtime, &state) {
                Ok(master) => master,
                Err(error) => {
                    return Err(format!("CROSS_PROJECT_SOURCE_REJECTED: {error}"));
                }
            };
            if master_holder.as_deref() != Some(from) {
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
                    "CROSS_PROJECT_SOURCE_REJECTED: source master has no registered route address"
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
        self.dispatch_sync_with_register_binding(project_context, req, None)
    }

    /// `dispatch_sync` with an optional outer-operation binder. When present
    /// and the request is a Register, the owner prepares the exact envelope,
    /// appends+syncs the outer `Validating` nested IDs, and only then consumes
    /// that same envelope. A sync failure returns an error and never consumes.
    fn dispatch_sync_with_register_binding(
        &self,
        project_context: Option<ProjectContext>,
        req: Req,
        outer_binding: Option<&RegisterOuterBinding>,
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
        if let Req::PeerLifecycle { ref request } = req {
            // Pure lifecycle requests never initialize a project runtime or
            // reconcile same-pane routes, leases, or registration state.
            let runtime = self.routes.lock().unwrap().get(&Self::route_key(&context))
                .and_then(|route| route.runtime.clone());
            let Some(runtime) = runtime else {
                return (self.host.clone(), peer_lifecycle::unavailable(request, "PEER_LIFECYCLE_ROUTE_UNAVAILABLE", false));
            };
            let response = match validate_request_context(&runtime, &req, Some(&context)) {
                Ok(()) => dispatch_with_route_context_bound(&runtime, req, Some(context), None),
                Err(error) => peer_lifecycle::unavailable(request, &error, true),
            };
            return (runtime, response);
        }
        if let Req::IdentityContext {
            facts,
            identity_context,
        } = req
        {
            let identity_context =
                identity_context.unwrap_or_else(|| IdentityContextRequest::legacy(facts));
            if identity_context.action == "prepare_invocation"
                || identity_context.action == "cancel"
            {
                return self.identity_context(context, identity_context);
            }
            if is_identity_query_request(&identity_context) {
                if !is_valid_identity_query_shape(&identity_context) {
                    return (
                        self.host.clone(),
                        Resp::err("IDENTITY_OPERATION_QUERY_SHAPE_INVALID"),
                    );
                }
                let response = self.operation_journal.query(
                    &identity_context,
                    &context.project_scope.as_str(),
                    context.app_scope_id.as_str(),
                );
                return (
                    self.host.clone(),
                    match response {
                        Ok(envelope) => {
                            // Read the exact inner receipt from the already-loaded
                            // runtime after outer authorization. This is a pure
                            // projection and never mutates state.
                            let nested_readback =
                                self.inner_register_receipt_readback(&context, &envelope.result);
                            let mut result = serde_json::to_value(&envelope.result)
                                .unwrap_or(serde_json::Value::Null);
                            if let (Some(object), Some(readback)) =
                                (result.as_object_mut(), nested_readback)
                            {
                                object.insert("nested_receipt".into(), readback);
                            }
                            Resp::data(json!({ "result": result }))
                        }
                        Err(error) => Resp::err(error),
                    },
                );
            }
            return self.identity_context(context, identity_context);
        }
        let register_binding = outer_binding.filter(|_| matches!(req, Req::Register { .. }));
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
                if let Err(error) = validate_request_context_with_register_approval(
                    &runtime,
                    &req,
                    Some(&context),
                    register_binding.and_then(RegisterOuterBinding::approval),
                ) {
                    Resp::err(error)
                } else {
                    dispatch_with_route_context_bound(
                        &runtime,
                        req,
                        Some(context.clone()),
                        register_binding,
                    )
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
            return (
                runtime,
                self.retire_superseded_claimants(superseded, response),
            );
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
                if let Err(error) = validate_request_context_with_register_approval(
                    &runtime,
                    &req,
                    Some(&context),
                    register_binding.and_then(RegisterOuterBinding::approval),
                ) {
                    Resp::err(error)
                } else {
                    dispatch_with_route_context_bound(
                        &runtime,
                        req,
                        Some(context.clone()),
                        register_binding,
                    )
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
            return (
                runtime,
                self.retire_superseded_claimants(superseded, response),
            );
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
                if let Err(error) = validate_request_context_with_register_approval(
                    &self.host,
                    &req,
                    Some(&context),
                    register_binding.and_then(RegisterOuterBinding::approval),
                ) {
                    Resp::err(error)
                } else {
                    dispatch_with_route_context_bound(
                        &self.host,
                        req,
                        Some(context.clone()),
                        register_binding,
                    )
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
            return (
                runtime,
                self.retire_superseded_claimants(superseded, response),
            );
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
        let response = if let Err(error) = validate_request_context_with_register_approval(
            &runtime,
            &req,
            Some(&context),
            register_binding.and_then(RegisterOuterBinding::approval),
        ) {
            Resp::err(error)
        } else {
            dispatch_with_route_context_bound(
                &runtime,
                req,
                Some(context.clone()),
                register_binding,
            )
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
        (
            runtime,
            self.retire_superseded_claimants(superseded, response),
        )
    }

}
