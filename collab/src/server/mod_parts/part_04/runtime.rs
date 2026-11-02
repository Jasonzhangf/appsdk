impl ProjectRuntimeManager {
    #[cfg(test)]
    fn ensure_resident_identity_context_route(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
    ) -> Result<(), String> {
        self.ensure_identity_context_route(context, request)
    }

    fn ensure_identity_context_route(
        &self,
        context: &ProjectContext,
        request: &IdentityContextRequest,
    ) -> Result<(), String> {
        if request.query
            || request.action == "cancel"
            || context.app_scope_id.as_str() != crate::identity::CLI_APP_SERVER_ID
        {
            return Ok(());
        }
        let context_root = Path::new(&context.canonical_root);
        let key = Self::route_key(context);
        if self
            .routes
            .lock()
            .unwrap()
            .get(&key)
            .and_then(|route| route.runtime.as_ref())
            .is_some()
        {
            return Ok(());
        }
        if !storage_roots_equal(context_root, &self.host_root)
            .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?
        {
            self.reserve_external_route(
                context,
                &self.storage_root_for_new(
                    context_root,
                    context.project_scope.as_str(),
                    context.app_scope_id.as_str(),
                ),
            )?;
            let storage_root = self
                .routes
                .lock()
                .unwrap()
                .get(&key)
                .map(|route| route.storage_root.clone())
                .unwrap_or_else(|| {
                    self.storage_root_for_new(
                        context_root,
                        context.project_scope.as_str(),
                        context.app_scope_id.as_str(),
                    )
                });
            self.ensure_runtime(&key, context_root, &storage_root)?;
            return Ok(());
        }
        let _admission = self.materialization_gate.lock().unwrap();
        if self.routes.lock().unwrap().contains_key(&key) {
            return Ok(());
        }
        self.append_resident_route_record(context)?;
        self.install_runtime(&key, self.host.clone(), None);
        Ok(())
    }

    fn reserve_external_route(
        &self,
        context: &ProjectContext,
        _central_storage_root: &Path,
    ) -> Result<(), String> {
        let key = Self::route_key(context);
        let project_root = Path::new(&context.canonical_root);
        let reservation = select_central_runtime_storage(
            &self.host.host_paths, &self.route_journal, &key, Some(&self.register_gate),
        ).map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?;
        let pending = self
            .routes
            .lock()
            .unwrap()
            .get(&key)
            .map(|route| (route.root.clone(), route.storage_root.clone()));
        if pending.is_some() {
            self.routes.lock().unwrap().entry(key).and_modify(|route| route.storage_root = reservation);
            return Ok(());
        }
        self.append_route_record(context, &reservation)?;
        self.install_pending_route(&key, project_root, &reservation);
        Ok(())
    }

    fn build_runtime(
        &self,
        key: &RouteKey,
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
            self.host.host_paths.state_root(),
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
                acquire_legacy_writer_lock(&server_dir.join("daemon.lock"), "project daemon")
                    .map_err(|error| format!("PROJECT_ROUTE_NOT_READY/UNSUPPORTED: {error}"))?,
            )
        };
        let journal_path = server_dir.join("journal.jsonl");
        let proven_pending = proven_pending_operations(
            self.host.host_paths.state_root(),
            key.0.as_str(),
            key.1.as_str(),
        )
        .map_err(|error| {
            format!(
                "PROJECT_ROUTE_NOT_READY/UNSUPPORTED: runtime control pending effects for {}: {error}",
                root.display()
            )
        })?;
        let state = replay_from_journal_with_proven(&root, &journal_path, &proven_pending).map_err(|error| {
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
            .and_modify(|route| {
                route.root = runtime.root.clone();
                route.storage_root = runtime.storage_root.clone();
                route.runtime = Some(runtime.clone());
            })
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
            self.host.host_paths.state_root(),
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
        let _publication = self.register_gate.lock().unwrap();
        let project_root = storage_owner_path(Path::new(&context.canonical_root))
            .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?;
        let host_state_root = storage_owner_path(self.host.host_paths.state_root())
            .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?;
        if project_root.starts_with(&host_state_root)
            && !storage_roots_equal(&project_root, &self.host_root)?
        {
            return Err("HOST_ROUTE_DURABILITY_FAILED: project root is inside the daemon runtime storage root; it cannot own another project's runtime assets".into());
        }
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
        if allow_resident_storage {
            for existing in &existing_records {
                if storage_roots_equal(Path::new(&existing.storage_root), &storage_root)
                    .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?
                {
                    return Err(format!(
                        "HOST_ROUTE_DURABILITY_FAILED: runtime storage root {} is already owned by route ({}, {})",
                        record.storage_root, existing.app_scope_id, existing.project_scope
                    ));
                }
            }
            let routes = self.routes.lock().unwrap();
            for ((app_scope, project_scope), route) in routes.iter() {
                if storage_roots_equal(&route.storage_root, &storage_root)
                    .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?
                {
                    return Err(format!(
                        "HOST_ROUTE_DURABILITY_FAILED: runtime storage root {} is already owned by route ({}, {})",
                        record.storage_root, app_scope, project_scope
                    ));
                }
            }
        } else if let Some(owner) = self
            .storage_owner(&storage_root, &existing_records)
            .map_err(|error| format!("HOST_ROUTE_DURABILITY_FAILED: {error}"))?
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

    /// Locate the one runtime for a source `RouteScope` carried by an
    /// authenticated `CommandEnvelope`. Routing by the envelope's exact
    /// (app scope, project scope) key keeps a caller from selecting among
    /// several app routes for the same project root.
    fn runtime_for_route_scope(&self, scope: &RouteScope) -> Result<Arc<Server>, String> {
        let key = (
            scope.app_scope_id.as_str().to_owned(),
            scope.project_scope_id.as_str().to_owned(),
        );
        let (root, storage_root) = {
            let routes = self.routes.lock().unwrap();
            let Some(route) = routes.get(&key) else {
                return Err(format!(
                    "CROSS_PROJECT_SOURCE_REJECTED: no registered source route for {} / {}",
                    scope.project_scope_id.as_str(),
                    scope.app_scope_id.as_str()
                ));
            };
            (route.root.clone(), route.storage_root.clone())
        };
        self.ensure_runtime(&key, &root, &storage_root)
            .map_err(|error| format!("CROSS_PROJECT_SOURCE_REJECTED: {error}"))
    }

    fn dispatch_cross_project_send(
        &self,
        target_context: &ProjectContext,
        req: Req,
    ) -> (Arc<Server>, Resp) {
        let Req::CrossProjectSend {
            from: _compat_from,
            from_project: _compat_from_project,
            worker_id,
            token,
            command,
            to,
            subject,
            body,
            in_reply_to,
            reply_policy,
            target_session,
        } = req
        else {
            unreachable!("cross-project dispatch requires CrossProjectSend");
        };
        let target = match self.select_runtime(target_context) {
            Ok(runtime) => runtime,
            Err(error) => return (self.host.clone(), Resp::err(error)),
        };
        if reply_policy.is_some() || target_session.is_some() {
            return (target, Resp::err("CROSS_PROJECT_OPTION_UNSUPPORTED: reply_policy and target_session are not supported"));
        }
        let Some(command) = command else {
            return (
                target,
                Resp::err(
                    "CROSS_PROJECT_CREDENTIALS_REQUIRED: cross-project send requires the caller command envelope",
                ),
            );
        };
        if worker_id.trim().is_empty() || token.is_empty() {
            return (
                target,
                Resp::err(
                    "CROSS_PROJECT_CREDENTIALS_REQUIRED: cross-project send requires the caller worker_id and token",
                ),
            );
        }
        let source = match self.runtime_for_route_scope(&command.scope) {
            Ok(source) => source,
            Err(error) => return (target, Resp::err(error)),
        };
        // Cross-project authorization and commit must be one linearized
        // boundary. The endpoint probe is the only fallible observation that
        // may block, so it runs before the state locks; reachability is not an
        // authorization input and a later probe result never revokes a durable
        // message.
        let recipient_snapshot = {
            let target_state = target.state.lock().unwrap();
            target_state.workers.get(&to).cloned()
        };
        if let Some(recipient) = recipient_snapshot {
            if worker_identity_and_endpoint(&*target, &recipient).1 == Some(false) {
                return (
                    target,
                    Resp::err(
                        "cross-project communication requires a reachable target binding on its server-selected transport",
                    ),
                );
            }
        }
        if Arc::ptr_eq(&source, &target) {
            return (
                target,
                Resp::err(
                    "CROSS_PROJECT_SAME_RUNTIME_REJECTED: source and target must be different projects",
                ),
            );
        }
        // Fixed lock order: the host state is always taken first, and the
        // host mutex serializes cross-project sends so source and target never
        // deadlock against each other. The same Arc is never locked twice.
        let mut host_state = self.host.state.lock().unwrap();
        if let Some(error) = &host_state.journal_poison {
            return (
                target,
                Resp::err(format!("HOST_AUTHORITY_STORE_UNAVAILABLE: {error}")),
            );
        }
        let ceo_grant = host_state.ceo_grant.clone();
        let committed = if Arc::ptr_eq(&self.host, &source) {
            let mut target_state = target.state.lock().unwrap();
            authorize_and_commit_cross_project(
                &self.host,
                &host_state,
                &target,
                &mut target_state,
                &command,
                &worker_id,
                &token,
                ceo_grant.as_ref(),
                &to,
                &subject,
                &body,
                in_reply_to.as_deref(),
            )
        } else if Arc::ptr_eq(&self.host, &target) {
            let source_state = source.state.lock().unwrap();
            authorize_and_commit_cross_project(
                &source,
                &source_state,
                &self.host,
                &mut host_state,
                &command,
                &worker_id,
                &token,
                ceo_grant.as_ref(),
                &to,
                &subject,
                &body,
                in_reply_to.as_deref(),
            )
        } else {
            let source_state = source.state.lock().unwrap();
            let mut target_state = target.state.lock().unwrap();
            authorize_and_commit_cross_project(
                &source,
                &source_state,
                &target,
                &mut target_state,
                &command,
                &worker_id,
                &token,
                ceo_grant.as_ref(),
                &to,
                &subject,
                &body,
                in_reply_to.as_deref(),
            )
        };
        drop(host_state);
        let response = match committed {
            Ok(committed) => notify_cross_project_committed(&*target, committed, &worker_id, &to),
            Err(response) => response,
        };
        (target, response)
    }

    /// One host-wide current CEO grant snapshot. The caller uses it only to
    /// select a target runtime; the authoritative read and holder resolution
    /// happen again under the host state lock before durable commit.
    fn ceo_grant_snapshot(&self) -> Result<Option<crate::ceo::CeoGrant>, String> {
        let state = self.host.state.lock().unwrap();
        if let Some(error) = &state.journal_poison {
            return Err(format!("HOST_AUTHORITY_STORE_UNAVAILABLE: {error}"));
        }
        Ok(state.ceo_grant.clone())
    }

    /// Locate the one runtime named by a host CEO grant. The route record
    /// already knows the canonical root and storage; opening it never changes
    /// the grant or the caller's workspace.
    fn runtime_for_ceo_grant(
        &self,
        grant: &crate::ceo::CeoGrant,
    ) -> Result<Arc<Server>, String> {
        let workspace = std::fs::canonicalize(&grant.workspace).map_err(|error| {
            format!("CEO_WORKSPACE_UNAVAILABLE: canonicalize grant workspace: {error}")
        })?;
        let key = (grant.app_scope_id.clone(), grant.workspace.clone());
        let (root, storage_root) = {
            let routes = self.routes.lock().unwrap();
            let Some(route) = routes.get(&key) else {
                return Err(
                    "CHANNEL_CEO_ROUTE_UNAVAILABLE: no registered route for the active CEO grant workspace"
                        .into(),
                );
            };
            if route.root != workspace {
                return Err(
                    "CHANNEL_CEO_ROUTE_UNAVAILABLE: registered CEO route root does not match the grant workspace"
                        .into(),
                );
            }
            (route.root.clone(), route.storage_root.clone())
        };
        self.ensure_runtime(&key, &root, &storage_root)
            .map_err(|error| format!("CHANNEL_CEO_ROUTE_UNAVAILABLE: {error}"))
    }

    fn channel_inbound_intent(
        &self,
        caller: &Arc<Server>,
        to: &str,
        body: &str,
        envelope: &crate::server::channel_envelope::ChannelEnvelope,
    ) -> Result<crate::server::ChannelInboundAddressIntent, String> {
        let state = caller.state.lock().unwrap();
        channel_inbound_address_intent(
            &state,
            &envelope.source_channel,
            &envelope.channel_instance,
            &envelope.external_conversation,
            body,
            to,
        )
        .map_err(|error| format!("CHANNEL_ROUTE_SCOPE_MISSING: {error}"))
    }

    /// Host-aware `ceo` channel-inbound dispatch. It reads the current host
    /// grant, verifies the caller is that grant workspace, selects the grant
    /// runtime, and commits under the fixed host-then-target lock order
    /// without ever passing a stale worker id.
    fn dispatch_channel_inbound_ceo(
        &self,
        caller: Arc<Server>,
        from: String,
        subject: String,
        envelope: crate::server::channel_envelope::ChannelEnvelope,
        intent: crate::server::ChannelInboundAddressIntent,
    ) -> (Arc<Server>, Resp) {
        let grant = match self.ceo_grant_snapshot() {
            Ok(Some(grant)) => grant,
            Ok(None) => {
                return (
                    caller,
                    Resp::err(
                        "CHANNEL_CEO_GRANT_MISSING: no typed CEO grant is active; [ceo] does not fall back to the project master",
                    ),
                )
            }
            Err(error) => return (caller, Resp::err(error)),
        };
        let caller_root = match std::fs::canonicalize(&caller.root) {
            Ok(root) => root,
            Err(error) => {
                return (
                    caller,
                    Resp::err(format!("CHANNEL_ROUTE_SCOPE_MISSING: {error}")),
                )
            }
        };
        let grant_workspace = match std::fs::canonicalize(&grant.workspace) {
            Ok(root) => root,
            Err(error) => {
                return (
                    caller,
                    Resp::err(format!("CHANNEL_CEO_WORKSPACE_UNAVAILABLE: {error}")),
                )
            }
        };
        // A message submitted from the former CEO workspace after the grant
        // moved is an explicit rejection. It must not be auto-routed to the
        // new grant holder.
        if caller_root != grant_workspace {
            return (
                caller,
                Resp::err(
                    "CHANNEL_CEO_STALE_WORKSPACE: the active CEO grant belongs to another workspace; do not send from the former CEO project",
                ),
            );
        }
        let target = match self.runtime_for_ceo_grant(&grant) {
            Ok(runtime) => runtime,
            Err(error) => return (caller, Resp::err(error)),
        };
        // The host grant is the single authority source. It is re-read under
        // the host state lock at commit time so a grant that moved after the
        // workspace check is never committed to the stale holder. When the
        // target is the host reducer one lock guards both authority and commit;
        // otherwise the fixed host-then-target order avoids any double lock.
        let mut response = if Arc::ptr_eq(&target, &self.host) {
            let mut host_state = self.host.state.lock().unwrap();
            if let Some(error) = &host_state.journal_poison {
                return (
                    caller,
                    Resp::err(format!("HOST_AUTHORITY_STORE_UNAVAILABLE: {error}")),
                );
            }
            let current_grant = host_state.ceo_grant.clone();
            channel_inbound_commit_with_state(
                &target,
                &mut host_state,
                from,
                subject,
                envelope,
                current_grant.as_ref(),
                &intent,
            )
        } else {
            // Fixed lock order: host state first, then the target runtime. A
            // single mutex is never locked twice.
            let mut host_state = self.host.state.lock().unwrap();
            if let Some(error) = &host_state.journal_poison {
                return (
                    caller,
                    Resp::err(format!("HOST_AUTHORITY_STORE_UNAVAILABLE: {error}")),
                );
            }
            let current_grant = host_state.ceo_grant.clone();
            let mut target_state = target.state.lock().unwrap();
            channel_inbound_commit_with_state(
                &target,
                &mut target_state,
                from,
                subject,
                envelope,
                current_grant.as_ref(),
                &intent,
            )
        };
        channel_inbound_finish(&target, &mut response);
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
        let decorate=matches!(&req,Req::Context {..}|Req::IdentityContext {..});
        let workspace_reset=matches!(&req,Req::ProjectReset{..});
        let context=project_context.clone();
        let (runtime,mut response)=self.dispatch_sync_with_register_binding_inner(project_context,req,outer_binding);
        if workspace_reset && response.ok {
            if let Err(error)=self.reconcile_ceo_workspace_reset(context.as_ref()) {
                response.ok=false;
                response.error=Some(format!("{}CEO_AUTHORITY_CLEANUP_FAILED: the project effect may already be committed; do not replay it: {}",response.error.as_ref().map(|e|format!("{e}; ")).unwrap_or_default(),error.error.unwrap_or_default()));
            }
        }
        if decorate && response.ok {
            self.decorate_ceo_context(context.as_ref(),&mut response.data);
        }
        (runtime,response)
    }

    fn dispatch_sync_with_register_binding_inner(
        &self,
        project_context: Option<ProjectContext>,
        req: Req,
        outer_binding: Option<&RegisterOuterBinding>,
    ) -> (Arc<Server>, Resp) {
        let Some(mut context) = project_context else {
            if let Req::Ceo { worker_id, token, command } = req {
                return self.dispatch_ceo(None, worker_id, token, command);
            }
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
        // `ceo` (and the bare project key) address the host-wide CEO grant, not
        // the caller's project-local grant. Resolve the caller runtime only to
        // parse the address intent; a typed CEO address goes through the
        // host-authority path below, every other address keeps the existing
        // local resolver unchanged.
        if let Req::ChannelInbound {
            from,
            to,
            subject,
            body,
            envelope,
        } = &req
        {
            if let Ok(caller) = self.select_runtime(&context) {
                if let Ok(intent) = self.channel_inbound_intent(&caller, to, body, envelope) {
                    let project_key = canonical_project_key(&caller.root);
                    let is_ceo = intent.address.eq_ignore_ascii_case("ceo")
                        || project_key
                            .as_deref()
                            .is_some_and(|key| intent.address.eq_ignore_ascii_case(key));
                    if is_ceo {
                        if let Err(error) = self.same_pane_master_route_ready(&caller) {
                            return (caller, Resp::err(error));
                        }
                        if let Err(error) =
                            validate_request_context(&caller, &req, Some(&context))
                        {
                            return (caller, Resp::err(error));
                        }
                        return self.dispatch_channel_inbound_ceo(
                            caller,
                            from.clone(),
                            subject.clone(),
                            envelope.clone(),
                            intent,
                        );
                    }
                }
            }
        }
        if let Req::Ceo {worker_id,token,command} = req {
            return self.dispatch_ceo(Some(context),worker_id,token,command);
        }
        if let Req::UnregisterSelf { worker_id, token } = &req {
            let runtime = self
                .routes
                .lock()
                .unwrap()
                .get(&Self::route_key(&context))
                .and_then(|route| route.runtime.clone());
            let Some(runtime) = runtime else {
                return (self.host.clone(), Resp::err("SELF_ROUTE_UNAVAILABLE"));
            };
            let control_binding = {
                let state = runtime.state.lock().unwrap();
                let route_scope = RouteScope {
                    app_scope_id: context.app_scope_id.clone(),
                    project_scope_id: context.project_scope.clone(),
                };
                state
                    .global
                    .lookup_project_for_route(&route_scope)
                    .and_then(|project| {
                        project
                            .runtime_bindings
                            .values()
                            .filter(|binding| binding.agent_id.as_str() == worker_id)
                            .max_by_key(|binding| binding.endpoint_generation)
                    })
                    .cloned()
            };
            let owned_routes: Vec<_> = self
                .host
                .state
                .lock()
                .unwrap()
                .global
                .current_thread_routes
                .values()
                .filter(|binding| {
                    binding.project_scope == context.project_scope
                        && binding.app_scope_id == context.app_scope_id
                        && binding.agent_id.as_str() == worker_id
                })
                .cloned()
                .collect();
            let mut response =
                peer_lifecycle::unregister_self(&runtime, Some(&context), worker_id, token);
            if response.ok {
                if let Some(binding) = control_binding.as_ref() {
                    match self.unregister_control_binding(&runtime, &context, worker_id, binding) {
                        Ok(crate::server::binding_store::Durability::Confirmed) => {}
                        Ok(crate::server::binding_store::Durability::Unknown) => {
                            Self::mark_control_durability_unknown(&mut response);
                        }
                        Err(error) => {
                            response.ok = false;
                            response.error = Some(error);
                        }
                    }
                }
            }
            if response.ok && !Arc::ptr_eq(&runtime, &self.host) {
                for binding in owned_routes {
                    if let Err(error) = self
                        .host
                        .commit_checked(&[Event::GlobalCurrentThreadRouteRetired { binding }])
                    {
                        response.ok = false;
                        response.error = Some(format!("SELF_ROUTE_CLEANUP_FAILED: {error}"));
                    }
                }
            }
            return (runtime, response);
        }
        if let Req::PeerLifecycle { ref request } = req {
            // Pure lifecycle requests never initialize a project runtime or
            // reconcile same-pane routes, leases, or registration state.
            let runtime = self
                .routes
                .lock()
                .unwrap()
                .get(&Self::route_key(&context))
                .and_then(|route| route.runtime.clone());
            let Some(runtime) = runtime else {
                return (
                    self.host.clone(),
                    peer_lifecycle::unavailable(request, "PEER_LIFECYCLE_ROUTE_UNAVAILABLE", false),
                );
            };
            let response = match validate_request_context(&runtime, &req, Some(&context)) {
                Ok(()) => {
                    dispatch_with_route_context_bound(&runtime, req, Some(context), None, None)
                }
                Err(error) => peer_lifecycle::unavailable(request, &error, true),
            };
            return (runtime, response);
        }
        if let Req::RegisterReply { reply } = req {
            return self.participant_register_reply(context, reply);
        }
        if let Req::IdentityContext {
            facts,
            identity_context,
        } = req
        {
            let identity_context =
                identity_context.unwrap_or_else(|| IdentityContextRequest::legacy(facts));
            if identity_context.action == "prepare_invocation" {
                if let Err(error) = self.ensure_identity_context_route(&context, &identity_context)
                {
                    return (self.host.clone(), Resp::err(error));
                }
                return self.identity_context(context, identity_context);
            }
            if identity_context.action == "cancel" {
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
                            if let Some(recovery) = self
                                .name_recovery_readback(&context, &identity_context.operation_id)
                            {
                                result["name_resolution"] = recovery;
                            }
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
            let mut register_effect_id = None;
            let mut register_control_first = self.register_control_first_hook(
                &runtime,
                register_binding.and_then(|binding| binding.name_pane_anchor().cloned()),
                &mut register_effect_id,
            );
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
                    Some(&mut register_control_first),
                )
            };
            drop(register_control_first);
            let (runtime, response) = self.finalize_registration(
                runtime,
                &context,
                register_worker_id.as_deref(),
                rollback_state.0,
                rollback_state.1,
                rollback_state.2,
                rollback_state.3,
                response,
                register_effect_id.as_deref(),
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
            let mut register_effect_id = None;
            let mut register_control_first = self.register_control_first_hook(
                &runtime,
                register_binding.and_then(|binding| binding.name_pane_anchor().cloned()),
                &mut register_effect_id,
            );
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
                    Some(&mut register_control_first),
                )
            };
            drop(register_control_first);
            let (runtime, response) = self.finalize_registration(
                runtime,
                &context,
                register_worker_id.as_deref(),
                rollback_state.0,
                rollback_state.1,
                rollback_state.2,
                rollback_state.3,
                response,
                register_effect_id.as_deref(),
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
                // The runtime reservation is already durable. Keep the same
                // resident reducer available when registration pauses at the
                // participant gate; continuation must not append it again.
                self.install_runtime(&key, self.host.clone(), None);
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
            let mut register_effect_id = None;
            let mut register_control_first = self.register_control_first_hook(
                &self.host,
                register_binding.and_then(|binding| binding.name_pane_anchor().cloned()),
                &mut register_effect_id,
            );
            let response = if let Err(error) = validate_request_context_with_register_approval(
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
                    Some(&mut register_control_first),
                )
            };
            drop(register_control_first);
            if !response.ok {
                return (self.host.clone(), response);
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
                register_effect_id.as_deref(),
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
        if let Err(error) = self.reserve_external_route(&context, &storage_root) {
            return (self.host.clone(), Resp::err(error));
        }
        let storage_root = self
            .routes
            .lock()
            .unwrap()
            .get(&key)
            .map(|route| route.storage_root.clone())
            .unwrap_or(storage_root);
        // Admit the project route before opening or mutating its reducer. The
        // host journal is the durable transaction boundary: if it cannot be
        // published, this request must not create or mutate a project
        // journal that would be unreachable after a restart.
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
        let mut register_effect_id = None;
        let mut register_control_first = self.register_control_first_hook(
            &runtime,
            register_binding.and_then(|binding| binding.name_pane_anchor().cloned()),
            &mut register_effect_id,
        );
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
                Some(&mut register_control_first),
            )
        };
        drop(register_control_first);
        let (runtime, response) = self.finalize_registration(
            runtime,
            &context,
            register_worker_id.as_deref(),
            rollback_state.0,
            rollback_state.1,
            rollback_state.2,
            rollback_state.3,
            response,
            register_effect_id.as_deref(),
        );
        (
            runtime,
            self.retire_superseded_claimants(superseded, response),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn authorize_and_commit_cross_project(
    source: &Server,
    source_state: &State,
    target: &Server,
    target_state: &mut State,
    command: &crate::proto::CommandEnvelope,
    worker_id: &str,
    token: &str,
    ceo_grant: Option<&crate::ceo::CeoGrant>,
    to: &str,
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
) -> Result<CrossProjectCommitted, Resp> {
    let Some(worker) = source_state.workers.get(worker_id).cloned() else {
        return Err(Resp::err(format!("worker {worker_id} not registered")));
    };
    if worker.token != token {
        return Err(Resp::err(
            "token mismatch: identity does not own this worker_id",
        ));
    }
    let Some(binding) = source_state
        .global
        .lookup_binding_for(&command.scope, &command.actor_binding_id)
        .cloned()
    else {
        return Err(Resp::err(
            "CROSS_PROJECT_BINDING_REJECTED: authenticated sender binding is not registered for the source route",
        ));
    };
    if binding.agent_id.as_str() != worker_id {
        return Err(Resp::err(
            "CROSS_PROJECT_BINDING_REJECTED: authenticated sender binding belongs to another worker",
        ));
    }
    let registered_scope = binding.route_scope();
    let runtime_identity = crate::identity::RuntimeIdentity {
        agent_id: binding.agent_id.clone(),
        runtime_id: binding.runtime_id.clone(),
        appserver_id: binding.app_scope_id.clone(),
        endpoint_generation: binding.endpoint_generation,
        binding_id: binding.binding_id.clone(),
        session_id: binding.session_id.clone(),
        native_thread_id: binding.native_thread_id.clone(),
    };
    if let Err(error) = command.validate_for(&runtime_identity, &registered_scope) {
        return Err(Resp::err(format!("CROSS_PROJECT_BINDING_REJECTED: {error}")));
    }
    let sender_role = cross_project_role(source, source_state, ceo_grant, worker_id)
        .map_err(Resp::err)?;
    let source_thread_id = source_state
        .workers
        .get(worker_id)
        .and_then(selected_transport_for_worker)
        .and_then(|transport| notification_target(&transport).map(str::to_owned));
    let source_project = source.root.to_string_lossy().into_owned();
    commit_cross_project_message(
        target,
        target_state,
        worker_id,
        &source_project,
        sender_role,
        ceo_grant,
        source_thread_id,
        to,
        subject,
        body,
        in_reply_to,
    )
}
