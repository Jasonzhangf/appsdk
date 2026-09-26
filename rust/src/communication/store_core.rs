use super::*;
use super::helpers::*;
use super::validation::*;

impl CommunicationStore {
    pub fn open(root: &Path) -> CommResult<Self> {
        validate_communication_root_input(root)?;
        if !root.exists() {
            fs::create_dir_all(root).map_err(|error| {
                CommError::new(
                    "communication_root_create_failed",
                    format!("{}: {error}", root.display()),
                )
            })?;
        }
        let canonical_root = validate_communication_root(root)?;
        Self::open_mailbox(canonical_root.join(".appsdk-control/communication/mailbox.jsonl"))
    }

    pub fn open_mailbox(mailbox_path: PathBuf) -> CommResult<Self> {
        let project_root = infer_project_root(&mailbox_path)?;
        Self::open_mailbox_at(mailbox_path, project_root)
    }

    pub(super) fn open_mailbox_read_only(mailbox_path: PathBuf) -> CommResult<Self> {
        let project_root = infer_project_root(&mailbox_path)?;
        reject_symlink_components(&mailbox_path, "communication_mailbox")?;
        if !mailbox_path.is_file() {
            return Err(CommError::new(
                "communication_mailbox_missing",
                format!(
                    "communication mailbox is missing: {}",
                    mailbox_path.display()
                ),
            ));
        }
        let mut store = Self {
            mailbox_path,
            project_root,
            projection: Projection::default(),
            _lock: CommunicationLock::read_only(),
        };
        store
            .projection
            .adapters
            .insert("mailbox".into(), default_mailbox_adapter());
        // A project store already holds its own exclusive mailbox lock while
        // resolving a cross-project target.  Replaying the target's complete
        // mailbox here would recursively acquire that lock (and can form an
        // A -> B -> A cycle).  Discovery only needs the target identity
        // projection; the target project performs full journal validation when
        // it is opened as the owner of its mailbox.
        store.replay_identity_only()?;
        Ok(store)
    }

    pub(super) fn open_mailbox_at(mailbox_path: PathBuf, project_root: PathBuf) -> CommResult<Self> {
        reject_symlink_components(&mailbox_path, "communication_mailbox")?;
        if let Some(parent) = mailbox_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                CommError::new(
                    "mailbox_directory_create_failed",
                    format!("{}: {error}", parent.display()),
                )
            })?;
        }
        reject_symlink_components(&mailbox_path, "communication_mailbox")?;
        let lock = CommunicationLock::acquire(&mailbox_path)?;
        let mut store = Self {
            mailbox_path,
            project_root,
            projection: Projection::default(),
            _lock: lock,
        };
        // The built-in mailbox adapter is part of the replay baseline. Events
        // created through the default adapter must validate against it while
        // replaying, before any journal-sourced adapter registrations apply.
        store
            .projection
            .adapters
            .insert("mailbox".into(), default_mailbox_adapter());
        store.replay()?;
        store.reconcile_discovery_pending()?;
        Ok(store)
    }

    pub fn status(&self) -> Value {
        let mut active_bugs: Vec<BugRecord> = self
            .projection
            .bugs
            .values()
            .filter(|bug| bug.status == "active")
            .cloned()
            .collect();
        active_bugs.sort_by(priority_then_time_bug);
        let mut loops: Vec<LoopRecord> = self.projection.loops.values().cloned().collect();
        loops.sort_by(|left, right| {
            if left.status == "active" && right.status != "active" {
                Ordering::Less
            } else if left.status != "active" && right.status == "active" {
                Ordering::Greater
            } else {
                self.loop_priority(left)
                    .cmp(&self.loop_priority(right))
                    .then_with(|| left.updated_at.cmp(&right.updated_at))
                    .then_with(|| left.loop_id.cmp(&right.loop_id))
            }
        });
        let pending: Vec<NotificationRecord> = self
            .projection
            .notifications
            .values()
            .filter(|notification| notification.status == "pending")
            .cloned()
            .collect();
        let emitted: Vec<NotificationRecord> = self
            .projection
            .notifications
            .values()
            .filter(|notification| notification.status == "emitted")
            .cloned()
            .collect();
        let unknown: Vec<NotificationRecord> = self
            .projection
            .notifications
            .values()
            .filter(|notification| notification.status == "unknown")
            .cloned()
            .collect();
        json!({
            "protocol": PROTOCOL,
            "mailboxPath": self.mailbox_path,
            "scopes": self.projection.scopes.values().collect::<Vec<_>>(),
            "agents": self.projection.agents.values().collect::<Vec<_>>(),
            "agentTombstones": self.projection.agent_tombstones.values().collect::<Vec<_>>(),
            "discoveryPending": self.projection.discovery_pending.values().collect::<Vec<_>>(),
            "messages": self.projection.messages.values().collect::<Vec<_>>(),
            "messageDeliveryAttempts": self
                .projection
                .message_delivery_attempts
                .values()
                .collect::<Vec<_>>(),
            "adapters": self.projection.adapters.values().collect::<Vec<_>>(),
            "activeBugs": active_bugs,
            "bugs": self.projection.bugs.values().collect::<Vec<_>>(),
            "loops": loops,
            "wakeup": self.projection.wakeup.values().collect::<Vec<_>>(),
            "masterWake": self.projection.master_wake.values().collect::<Vec<_>>(),
            "notificationProjection": {
                "pending": pending,
                "emitted": emitted,
                "unknown": unknown,
                "batches": self.projection.batches
            }
        })
    }

    pub(super) fn loop_priority(&self, loop_record: &LoopRecord) -> Priority {
        loop_record
            .loop_id
            .strip_prefix("bug-loop-")
            .and_then(|bug_id| self.projection.bugs.get(bug_id))
            .map(|bug| bug.priority.clone())
            .unwrap_or(Priority::P3)
    }

    pub(super) fn validate_project_root(&self, project_root: &str) -> CommResult<()> {
        let requested = Path::new(project_root);
        if !is_lexically_canonical_absolute(requested) {
            return Err(CommError::new(
                "project_root_not_canonical",
                format!("projectRoot must be an absolute canonical path: {project_root}"),
            ));
        }
        reject_symlink_components(requested, "communication_project_root")?;
        if requested != self.project_root {
            return Err(CommError::new(
                "project_root_mismatch",
                format!(
                    "projectRoot does not match communication root: expected {}, got {}",
                    self.project_root.display(),
                    requested.display()
                ),
            ));
        }
        Ok(())
    }

    pub(super) fn register_runtime(&mut self, request: RuntimeRequest) -> CommResult<Value> {
        self.validate_project_root(&request.project_root)?;
        let identity = global_registry::RuntimeIdentity {
            runtime_id: request.runtime_id,
            appserver_id: request.appserver_id,
            namespace: request.namespace,
            endpoint: request.endpoint,
            project_root: request.project_root,
            capabilities: request.capabilities,
            process_id: request.process_id,
        };
        let receipt = global_registry::register_runtime(&identity).map_err(|error| {
            CommError::new(
                if error.starts_with("GLOBAL_RUNTIME_IDENTITY_CONFLICT:") {
                    "runtime_identity_conflict"
                } else if error.starts_with("GLOBAL_RUNTIME_NOT_FOUND:") {
                    "runtime_not_found"
                } else {
                    "runtime_registration_failed"
                },
                error,
            )
        })?;
        Ok(json!({
            "runtime": identity,
            "receipt": serde_json::to_value(receipt).unwrap(),
            "registry": "host"
        }))
    }

    pub(super) fn require_runtime_for_scope(
        &self,
        request: &ScopeRequest,
    ) -> CommResult<global_registry::RuntimeRecord> {
        let runtime_id = request.runtime_id.as_deref().ok_or_else(|| {
            CommError::new(
                "runtime_registration_required",
                "scope registration requires a host runtimeId registered in ~/.appsdk",
            )
        })?;
        let runtime = global_registry::runtime(runtime_id)
            .map_err(|error| CommError::new("runtime_registration_required", error))?;
        if runtime.identity.appserver_id != request.appserver_id
            || runtime.identity.namespace != request.namespace
            || runtime.identity.endpoint != request.endpoint
            || runtime.identity.project_root != request.project_root
        {
            return Err(CommError::new(
                "runtime_scope_mismatch",
                format!(
                    "runtime {} does not match scope transport identity",
                    runtime_id
                ),
            ));
        }
        Ok(runtime)
    }

    pub(super) fn require_runtime_for_agent(
        &self,
        scope: &ScopeRecord,
        runtime_id: Option<&str>,
    ) -> CommResult<global_registry::RuntimeRecord> {
        let expected = scope.runtime_id.as_deref().ok_or_else(|| {
            CommError::new(
                "runtime_registration_required",
                "agent registration requires a scope bound to a host runtime",
            )
        })?;
        let runtime_id = runtime_id.ok_or_else(|| {
            CommError::new(
                "runtime_registration_required",
                "agent registration requires runtimeId",
            )
        })?;
        if runtime_id != expected {
            return Err(CommError::new(
                "runtime_agent_mismatch",
                format!("agent runtimeId does not match scope runtimeId: {runtime_id}"),
            ));
        }
        global_registry::runtime(runtime_id)
            .map_err(|error| CommError::new("runtime_registration_required", error))
    }

    pub(super) fn runtime_for_agent(&self, agent: &AgentRecord) -> CommResult<global_registry::RuntimeRecord> {
        let runtime_id = agent.runtime_id.as_deref().ok_or_else(|| {
            CommError::new(
                "runtime_registration_required",
                format!(
                    "agent has no verified runtime identity: {}",
                    agent.address().key()
                ),
            )
        })?;
        global_registry::runtime(runtime_id)
            .map_err(|error| CommError::new("runtime_registration_required", error))
    }

    pub(super) fn require_agent_runtime(&self, agent: &AgentRecord) -> CommResult<()> {
        self.runtime_for_agent(agent).map(|_| ())
    }

    pub(super) fn global_address(address: &Address) -> global_registry::CommunicationAddress {
        global_registry::CommunicationAddress {
            scope_id: address.scope_id.clone(),
            session_id: address.session_id.clone(),
        }
    }

    pub(super) fn discovery_registration_error(error: String) -> CommError {
        CommError::new(
            "communication_discovery_registration_failed",
            format!("global communication discovery registration failed: {error}"),
        )
    }

    pub(super) fn discovery_pending_error(mut error: CommError, pending_id: &str) -> CommError {
        let cause = error.context;
        error.context = json!({
            "pendingId": pending_id,
            "cause": cause
        });
        error
    }

    pub(super) fn discovery_recovery_error(
        pending: &DiscoveryPendingRecord,
        error: impl Into<String>,
    ) -> CommError {
        let mut failure = CommError::new(
            "communication_discovery_recovery_failed",
            format!(
                "unable to reconcile host communication discovery for pending operation {}: {}",
                pending.pending_id,
                error.into()
            ),
        );
        failure.context = json!({
            "pendingId": &pending.pending_id,
            "operation": &pending.operation,
        });
        failure
    }

    pub(super) fn begin_discovery(&mut self, operation: DiscoveryOperation) -> CommResult<String> {
        let pending = DiscoveryPendingRecord {
            pending_id: new_id("discovery"),
            operation,
            created_at: now(),
        };
        let pending_id = pending.pending_id.clone();
        self.commit(
            "discovery.pending",
            serde_json::to_value(&pending).expect("discovery pending record is serializable"),
        )?;
        Ok(pending_id)
    }

    pub(super) fn publish_discovery(&self, operation: &DiscoveryOperation) -> Result<(), String> {
        match operation {
            DiscoveryOperation::Scope { record } => {
                global_registry::register_communication_scope(
                    &record.scope_id,
                    Path::new(&record.project_root),
                )?;
            }
            DiscoveryOperation::Agent { record } => {
                let scope = self
                    .projection
                    .scopes
                    .get(&record.scope_id)
                    .ok_or_else(|| "scope is missing while publishing agent".to_string())?;
                global_registry::register_communication_agent(
                    &record.scope_id,
                    &record.session_id,
                    Path::new(&scope.project_root),
                )?;
            }
            DiscoveryOperation::Rebind { event } => {
                let scope = self
                    .projection
                    .scopes
                    .get(&event.to.scope_id)
                    .ok_or_else(|| "scope is missing while publishing rebind".to_string())?;
                global_registry::rebind_communication_agent(
                    &event.from.scope_id,
                    &event.from.session_id,
                    &event.to.session_id,
                    Path::new(&scope.project_root),
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn finish_discovery(&mut self, pending_id: &str) -> CommResult<()> {
        if !self.projection.discovery_pending.contains_key(pending_id) {
            return Err(CommError::new(
                "discovery_pending_missing",
                format!("discovery pending operation is missing: {pending_id}"),
            ));
        }
        self.commit("discovery.reconciled", json!({ "pendingId": pending_id }))
            .map(|_| ())
    }

    pub(super) fn publish_and_finish(
        &mut self,
        pending_id: &str,
        operation: &DiscoveryOperation,
    ) -> CommResult<()> {
        if let Err(error) = self.publish_discovery(operation) {
            return Err(Self::discovery_pending_error(
                Self::discovery_registration_error(error),
                pending_id,
            ));
        }
        self.finish_discovery(pending_id)
            .map_err(|error| Self::discovery_pending_error(error, pending_id))
    }

    pub(super) fn ensure_local_discovery_operation(
        &mut self,
        operation: &DiscoveryOperation,
    ) -> CommResult<()> {
        match operation {
            DiscoveryOperation::Scope { record } => {
                match self.projection.scopes.get(&record.scope_id) {
                    None => {
                        self.commit(
                            "scope.registered",
                            serde_json::to_value(record).expect("scope record is serializable"),
                        )?;
                    }
                    Some(existing) if existing == record => {}
                    Some(_) => {
                        return Err(CommError::new(
                            "discovery_recovery_conflict",
                            format!(
                                "scope changed while discovery was pending: {}",
                                record.scope_id
                            ),
                        ));
                    }
                }
            }
            DiscoveryOperation::Agent { record } => {
                let key = record.address().key();
                match self.projection.agents.get(&key) {
                    None if self.projection.agent_tombstones.contains_key(&key) => {
                        return Err(CommError::new(
                            "discovery_recovery_conflict",
                            format!("agent address is already rebound: {key}"),
                        ));
                    }
                    None => {
                        self.require_scope(&record.scope_id)?;
                        self.commit(
                            "agent.registered",
                            serde_json::to_value(record).expect("agent record is serializable"),
                        )?;
                    }
                    Some(existing) if existing == record => {}
                    Some(_) => {
                        return Err(CommError::new(
                            "discovery_recovery_conflict",
                            format!("agent changed while discovery was pending: {key}"),
                        ));
                    }
                }
            }
            DiscoveryOperation::Rebind { event } => {
                let from_key = event.from.address().key();
                let to_key = event.to.address().key();
                let local_rebound = self
                    .projection
                    .agents
                    .get(&to_key)
                    .is_some_and(|agent| agent == &event.to)
                    && self
                        .projection
                        .agent_tombstones
                        .get(&from_key)
                        .is_some_and(|tombstone| tombstone == &event.tombstone);
                if local_rebound {
                    return Ok(());
                }
                if self.projection.agents.get(&from_key) != Some(&event.from)
                    || self.projection.agents.contains_key(&to_key)
                    || self.projection.agent_tombstones.contains_key(&from_key)
                    || self.projection.agent_tombstones.contains_key(&to_key)
                {
                    return Err(CommError::new(
                        "discovery_recovery_conflict",
                        format!(
                            "agent rebind state changed while discovery was pending: {from_key}"
                        ),
                    ));
                }
                self.commit(
                    "agent.rebound",
                    serde_json::to_value(event).expect("agent rebound event is serializable"),
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn reconcile_discovery_pending(&mut self) -> CommResult<()> {
        let pending: Vec<DiscoveryPendingRecord> = self
            .projection
            .discovery_pending
            .values()
            .cloned()
            .collect();
        for record in pending {
            if let Err(error) = self.ensure_local_discovery_operation(&record.operation) {
                return Err(Self::discovery_recovery_error(&record, error.to_string()));
            }
            if let Err(error) = self.publish_discovery(&record.operation) {
                return Err(Self::discovery_recovery_error(&record, error));
            }
            self.finish_discovery(&record.pending_id)
                .map_err(|error| Self::discovery_recovery_error(&record, error.to_string()))?;
        }
        Ok(())
    }

    pub(super) fn discovery_lookup_error(error: String) -> CommError {
        CommError::new(
            "communication_discovery_failed",
            format!("global communication discovery lookup failed: {error}"),
        )
    }

    pub(super) fn target_mailbox_root(target: &global_registry::CommunicationTarget) -> CommResult<PathBuf> {
        let mailbox = target
            .project_root
            .join(".appsdk-control/communication/mailbox.jsonl");
        if !mailbox.is_file() {
            return Err(CommError::new(
                "communication_target_mailbox_missing",
                format!(
                    "registered target mailbox is missing: {}",
                    mailbox.display()
                ),
            ));
        }
        Ok(target.project_root.clone())
    }

    pub(super) fn resolve_agent(&self, address: &Address) -> CommResult<AgentRecord> {
        let key = address.key();
        if self.projection.agents.contains_key(&key)
            || self.projection.agent_tombstones.contains_key(&key)
        {
            return self.require_agent(address).cloned();
        }

        let target = global_registry::communication_target(&Self::global_address(address))
            .map_err(Self::discovery_lookup_error)?
            .ok_or_else(|| {
                CommError::new(
                    "agent_not_registered",
                    format!("agent not registered: {}", address.key()),
                )
            })?;
        if let Some(rebound_from) = target.rebound_from.as_ref() {
            let mut error = CommError::new(
                "agent_address_rebound",
                format!("agent address was rebound to {}", target.address.session_id),
            );
            error.context = json!({
                "oldAddress": rebound_from,
                "newAddress": target.address,
                "projectRoot": target.project_root,
            });
            return Err(error);
        }
        let project_root = Self::target_mailbox_root(&target)?;
        if project_root == self.project_root {
            return Err(CommError::new(
                "agent_not_registered",
                format!("agent not registered: {}", address.key()),
            ));
        }
        let external = Self::open_mailbox_read_only(
            project_root.join(".appsdk-control/communication/mailbox.jsonl"),
        )
        .map_err(|mut error| {
            error.context = json!({
                "targetAddress": address,
                "targetProjectRoot": project_root,
                "cause": error.context,
            });
            error
        })?;
        let external_address = Address {
            scope_id: target.address.scope_id.clone(),
            session_id: target.address.session_id.clone(),
        };
        external.require_agent(&external_address).cloned()
    }

    pub(super) fn resolve_live_agent(&self, address: &Address) -> CommResult<AgentRecord> {
        let agent = self.resolve_agent(address)?;
        if !agent.live_at(&now()) {
            return Err(CommError::new(
                "agent_lease_expired",
                format!("agent lease expired: {}", address.key()),
            ));
        }
        Ok(agent)
    }

    pub(super) fn resolve_scope_for_agent(&self, agent: &AgentRecord) -> CommResult<ScopeRecord> {
        if let Some(scope) = self.projection.scopes.get(&agent.scope_id) {
            return Ok(scope.clone());
        }
        let target = global_registry::communication_target(&Self::global_address(&agent.address()))
            .map_err(Self::discovery_lookup_error)?
            .ok_or_else(|| {
                CommError::new(
                    "scope_not_found",
                    format!("scope not found: {}", agent.scope_id),
                )
            })?;
        if target.rebound_from.is_some() {
            return Err(CommError::new(
                "agent_address_rebound",
                format!("agent address was rebound: {}", agent.address().key()),
            ));
        }
        let project_root = Self::target_mailbox_root(&target)?;
        if project_root == self.project_root {
            return Err(CommError::new(
                "scope_not_found",
                format!("scope not found: {}", agent.scope_id),
            ));
        }
        let external = Self::open_mailbox_read_only(
            project_root.join(".appsdk-control/communication/mailbox.jsonl"),
        )
        .map_err(|mut error| {
            error.context = json!({
                "targetAddress": agent.address(),
                "targetProjectRoot": project_root,
                "cause": error.context,
            });
            error
        })?;
        external.require_scope(&agent.scope_id).cloned()
    }

    pub(super) fn register_adapter(&mut self, request: AdapterRequest) -> CommResult<Value> {
        validate_non_empty(&request.adapter_id, "adapterId")?;
        if !matches!(request.kind.as_str(), "mailbox" | "appserver") {
            return Err(CommError::new(
                "invalid_adapter_kind",
                format!(
                    "adapter kind must be mailbox or appserver: {}",
                    request.kind
                ),
            ));
        }
        if request.kind == "appserver" && request.target.as_deref().unwrap_or("").trim().is_empty()
        {
            return Err(CommError::new(
                "appserver_target_required",
                "appserver adapter requires endpoint",
            ));
        }
        if request.kind != "mailbox" && request.recipient.is_none() {
            return Err(CommError::new(
                "adapter_recipient_required",
                "appserver adapters require a registered recipient address",
            ));
        }
        if let Some(recipient) = request.recipient.as_ref() {
            let agent = self.resolve_live_agent(recipient)?;
            if request.kind != "mailbox" {
                let runtime = self.runtime_for_agent(&agent)?;
                let target = request.target.as_deref().ok_or_else(|| {
                    CommError::new(
                        "appserver_target_required",
                        "appserver adapter requires endpoint",
                    )
                })?;
                validate_adapter_runtime_target(&request.kind, target, &runtime, false)?;
            }
        }
        let record = AdapterRecord {
            adapter_id: request.adapter_id.clone(),
            kind: request.kind,
            target: request.target,
            enabled: request.enabled.unwrap_or(true),
            execute: request.execute.unwrap_or(false),
            recipient: request.recipient,
            registered_at: now(),
        };
        if let Some(existing) = self.projection.adapters.get(&record.adapter_id) {
            if existing.kind == record.kind
                && existing.target == record.target
                && existing.enabled == record.enabled
                && existing.execute == record.execute
                && existing.recipient == record.recipient
            {
                return Ok(json!({ "adapter": existing, "idempotent": true }));
            }
            return Err(CommError::new(
                "adapter_conflict",
                format!("adapter already registered: {}", record.adapter_id),
            ));
        }
        self.commit("adapter.registered", serde_json::to_value(&record).unwrap())?;
        Ok(json!({ "adapter": record, "idempotent": false }))
    }

    pub(super) fn register_scope(&mut self, request: ScopeRequest) -> CommResult<Value> {
        validate_scope_request(&request)?;
        self.validate_project_root(&request.project_root)?;
        let _runtime = self.require_runtime_for_scope(&request)?;
        let at = now();
        if let Some(existing) = self.projection.scopes.get(&request.scope_id).cloned() {
            if existing.appserver_id == request.appserver_id
                && existing.namespace == request.namespace
                && existing.endpoint == request.endpoint
                && existing.project_root == request.project_root
                && existing.session_ids == request.session_ids
                && existing.runtime_id.as_deref() == request.runtime_id.as_deref()
            {
                let operation = DiscoveryOperation::Scope {
                    record: existing.clone(),
                };
                let pending_id = self.begin_discovery(operation.clone())?;
                self.publish_and_finish(&pending_id, &operation)?;
                return Ok(json!({ "scope": existing, "idempotent": true }));
            }
            return Err(CommError::new(
                "scope_conflict",
                format!(
                    "scope already registered with different identity: {}",
                    request.scope_id
                ),
            ));
        }
        let record = ScopeRecord {
            scope_id: request.scope_id.clone(),
            appserver_id: request.appserver_id,
            namespace: request.namespace,
            endpoint: request.endpoint,
            project_root: request.project_root,
            session_ids: request.session_ids,
            registered_at: at.clone(),
            last_observed_at: at,
            master_session_id: None,
            runtime_id: request.runtime_id,
        };
        let operation = DiscoveryOperation::Scope {
            record: record.clone(),
        };
        let pending_id = self.begin_discovery(operation.clone())?;
        if let Err(error) = self.commit("scope.registered", serde_json::to_value(&record).unwrap())
        {
            return Err(Self::discovery_pending_error(error, &pending_id));
        }
        self.publish_and_finish(&pending_id, &operation)?;
        Ok(json!({ "scope": record, "idempotent": false }))
    }

    pub(super) fn register_agent(&mut self, request: AgentRequest) -> CommResult<Value> {
        validate_non_empty(&request.scope_id, "scopeId")?;
        validate_non_empty(&request.session_id, "sessionId")?;
        validate_non_empty(&request.agent_id, "agentId")?;
        let scope = self.require_scope(&request.scope_id)?.clone();
        let role = request.role.clone().unwrap_or_else(|| "peer".into());
        if role == "auto" {
            return Err(CommError::new(
                "role_auto_forbidden",
                "agent role auto is forbidden; register peer or provide explicit masterGrant",
            ));
        }
        if !matches!(role.as_str(), "master" | "peer" | "subagent") {
            return Err(CommError::new(
                "invalid_agent_role",
                format!("unsupported agent role: {role}"),
            ));
        }
        self.require_runtime_for_agent(&scope, request.runtime_id.as_deref())?;
        if !scope.session_ids.is_empty() && !scope.session_ids.contains(&request.session_id) {
            return Err(CommError::new(
                "session_not_declared",
                format!(
                    "session is not declared in scope: {}/{}",
                    request.scope_id, request.session_id
                ),
            ));
        }
        if role == "master"
            && request
                .master_grant
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            return Err(CommError::new(
                "master_grant_required",
                "master registration requires non-empty user masterGrant",
            ));
        }
        if role == "subagent" {
            let parent = request.parent.as_ref().ok_or_else(|| {
                CommError::new(
                    "subagent_parent_required",
                    "subagent registration requires parent",
                )
            })?;
            let parent_agent = self.require_agent(parent)?.clone();
            if parent.scope_id != request.scope_id {
                return Err(CommError::new(
                    "subagent_parent_scope_mismatch",
                    "subagent parent must be in the same scope",
                ));
            }
            if !parent_agent.live_at(&now()) {
                return Err(CommError::new(
                    "subagent_parent_expired",
                    "subagent parent lease is expired",
                ));
            }
        } else if request.parent.is_some() {
            return Err(CommError::new(
                "parent_only_for_subagent",
                "parent is only valid for a subagent",
            ));
        }
        let key = Address {
            scope_id: request.scope_id.clone(),
            session_id: request.session_id.clone(),
        }
        .key();
        if self.projection.agent_tombstones.contains_key(&key) {
            let mut error = CommError::new(
                "agent_address_rebound",
                format!("agent address is a read-only rebound tombstone: {key}"),
            );
            error.context = json!({
                "oldAddress": {
                    "scopeId": request.scope_id,
                    "sessionId": request.session_id
                },
                "tombstone": self.projection.agent_tombstones.get(&key)
            });
            return Err(error);
        }
        if let Some(existing) = self.projection.agents.get(&key).cloned() {
            if existing.role == role
                && existing.agent_id == request.agent_id
                && existing.parent == request.parent
                && existing.runtime_id.as_deref() == request.runtime_id.as_deref()
            {
                let operation = DiscoveryOperation::Agent {
                    record: existing.clone(),
                };
                let pending_id = self.begin_discovery(operation.clone())?;
                self.publish_and_finish(&pending_id, &operation)?;
                let reconciled_idle = if existing.role == "master" {
                    self.reconcile_idle_workers_for_master(&existing)?
                } else {
                    Vec::new()
                };
                return Ok(json!({
                    "agent": existing,
                    "idempotent": true,
                    "reconciledIdle": reconciled_idle
                }));
            }
            return Err(CommError::new(
                "agent_conflict",
                format!("agent address already registered: {key}"),
            ));
        }
        if role == "master"
            && scope
                .master_session_id
                .as_deref()
                .is_some_and(|master| master != request.session_id)
        {
            return Err(CommError::new(
                "master_already_registered",
                format!(
                    "scope already has a master: {}",
                    scope.master_session_id.unwrap()
                ),
            ));
        }
        let at = now();
        let lease_ms = request
            .lease_ms
            .unwrap_or(DEFAULT_AGENT_LEASE_MS)
            .max(1_000);
        let expires_at = add_millis(&at, lease_ms as i64)?;
        let record = AgentRecord {
            scope_id: request.scope_id.clone(),
            session_id: request.session_id.clone(),
            agent_id: request.agent_id,
            role,
            master_grant: request.master_grant,
            parent: request.parent,
            lease_ms,
            registered_at: at.clone(),
            last_observed_at: at.clone(),
            expires_at,
            state: AgentState::Working,
            last_state_at: at,
            runtime_id: request.runtime_id,
        };
        let operation = DiscoveryOperation::Agent {
            record: record.clone(),
        };
        let pending_id = self.begin_discovery(operation.clone())?;
        if let Err(error) = self.commit("agent.registered", serde_json::to_value(&record).unwrap())
        {
            return Err(Self::discovery_pending_error(error, &pending_id));
        }
        self.publish_and_finish(&pending_id, &operation)?;
        let reconciled_idle = if record.role == "master" {
            self.reconcile_idle_workers_for_master(&record)?
        } else {
            Vec::new()
        };
        Ok(json!({
            "agent": record,
            "idempotent": false,
            "reconciledIdle": reconciled_idle
        }))
    }

    pub(super) fn reconcile_idle_workers_for_master(
        &mut self,
        master: &AgentRecord,
    ) -> CommResult<Vec<Value>> {
        let observed_at = now();
        if !master.live_at(&observed_at) {
            return Ok(Vec::new());
        }
        let workers: Vec<AgentRecord> = self
            .projection
            .agents
            .values()
            .filter(|agent| {
                agent.scope_id == master.scope_id
                    && agent.role != "master"
                    && agent.state == AgentState::Idle
                    && agent.live_at(&observed_at)
            })
            .cloned()
            .collect();
        let master_address = master.address();
        let mut reconciled = Vec::new();
        for worker in workers {
            let signal_key = worker_idle_signal_key(&worker.address());
            let message =
                worker_idle_message(&worker, master_address.clone(), &worker.last_state_at);
            let message_id = message
                .message_id
                .as_deref()
                .expect("worker idle message must have a deterministic message id");
            let notification_key = structured_key(&[
                &worker.address().key(),
                &master_address.key(),
                "mailbox",
                &format!("idle:{}", worker.address().key()),
            ]);
            let signal_recorded =
                self.master_wake_signal_recorded(&master_address, &signal_key, message_id);
            let signal_consumed =
                self.master_wake_signal_consumed(&master_address, &signal_key, message_id);
            let notification_matches_message = self
                .projection
                .notifications
                .get(&notification_key)
                .is_some_and(|notification| notification.message_id == message_id);
            if !signal_recorded {
                self.accumulate_worker_idle(&worker, &master_address, &worker.last_state_at)?;
            }
            if !signal_consumed
                && (!self.projection.messages.contains_key(message_id)
                    || !notification_matches_message)
            {
                // send() persists the deterministic message and its
                // notification.  It is safe to call after a partial prefix:
                // messageId and the idle edge are the idempotency boundary.
                reconciled.push(self.send(message)?);
            }
        }
        Ok(reconciled)
    }

    pub fn refresh_agent(&mut self, address: Address, at: Option<&str>) -> CommResult<Value> {
        let at = at.map(validate_time).transpose()?.unwrap_or_else(now);
        let current = self.require_agent(&address)?.clone();
        let refreshed = AgentRecord {
            last_observed_at: at.clone(),
            expires_at: add_millis(&at, current.lease_ms as i64)?,
            ..current
        };
        self.commit("agent.refreshed", serde_json::to_value(&refreshed).unwrap())?;
        Ok(json!({ "agent": refreshed, "observedAt": at }))
    }

    pub(super) fn rebind_agent(&mut self, request: RebindAgentRequest) -> CommResult<Value> {
        validate_address(&request.from)?;
        validate_address(&request.to)?;
        validate_non_empty(&request.runtime_id, "runtimeId")?;
        if request.from.scope_id != request.to.scope_id {
            return Err(CommError::new(
                "agent_rebind_scope_mismatch",
                "agent rebind must keep the same scope",
            ));
        }
        if request.from == request.to {
            return Err(CommError::new(
                "agent_address_occupied",
                "agent rebind target must use a new session address",
            ));
        }

        let current = self.require_live_agent(&request.from)?.clone();
        if current.runtime_id.as_deref() != Some(request.runtime_id.as_str()) {
            let mut error = CommError::new(
                "agent_rebind_runtime_mismatch",
                format!(
                    "agent runtimeId does not match rebind runtimeId: {}",
                    request.runtime_id
                ),
            );
            error.context = json!({
                "address": request.from,
                "agentId": current.agent_id,
                "agentRuntimeId": current.runtime_id,
                "requestedRuntimeId": request.runtime_id
            });
            return Err(error);
        }
        let scope = self.require_scope(&request.from.scope_id)?.clone();
        self.require_runtime_for_agent(&scope, Some(request.runtime_id.as_str()))?;
        if !scope.session_ids.is_empty() && !scope.session_ids.contains(&request.to.session_id) {
            return Err(CommError::new(
                "session_not_declared",
                format!(
                    "session is not declared in scope: {}/{}",
                    request.to.scope_id, request.to.session_id
                ),
            ));
        }
        if self.projection.agents.contains_key(&request.to.key())
            || self
                .projection
                .agent_tombstones
                .contains_key(&request.to.key())
        {
            let mut error = CommError::new(
                "agent_address_occupied",
                format!(
                    "agent rebind target is already occupied: {}",
                    request.to.key()
                ),
            );
            error.context = json!({ "address": request.to });
            return Err(error);
        }
        if current.role == "master"
            && scope.master_session_id.as_deref() != Some(current.session_id.as_str())
        {
            return Err(CommError::new(
                "master_registration_state_invalid",
                "registered master address does not match the scope master session",
            ));
        }

        let at = request
            .at
            .map(|value| validate_time(&value))
            .transpose()?
            .unwrap_or_else(now);
        let rebound = AgentRecord {
            session_id: request.to.session_id.clone(),
            last_observed_at: at.clone(),
            expires_at: add_millis(&at, current.lease_ms as i64)?,
            ..current.clone()
        };
        let tombstone = AgentTombstone {
            address: request.from.clone(),
            agent_id: current.agent_id.clone(),
            runtime_id: request.runtime_id,
            rebound_to: rebound.address(),
            rebound_at: at,
        };
        let event = AgentReboundEvent {
            from: current,
            to: rebound.clone(),
            tombstone: tombstone.clone(),
        };
        let data = serde_json::to_value(&event).unwrap();
        let operation = DiscoveryOperation::Rebind {
            event: event.clone(),
        };
        let pending_id = self.begin_discovery(operation.clone())?;
        if let Err(error) = self.commit("agent.rebound", data) {
            return Err(Self::discovery_pending_error(error, &pending_id));
        }
        self.publish_and_finish(&pending_id, &operation)?;
        Ok(json!({
            "agent": rebound,
            "tombstone": tombstone,
            "idempotent": false
        }))
    }
}
