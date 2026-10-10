/// The receipt graph required by the migration apply gate. The optional
/// writer is intentional: a missing writer remains a typed validation error,
/// rather than being represented by an invented default writer.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MigrationReceiptSet {
    pub writer: Option<MigrationWriterReceipt>,
    #[serde(default)]
    pub identity_rebinds: Vec<MigrationIdentityRebindReceipt>,
    #[serde(default)]
    pub runtime_rebinds: Vec<MigrationRuntimeRebindReceipt>,
}

impl MigrationReceiptSet {
    pub fn validate(&self) -> Result<(), StateError> {
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| StateError::invalid("migration writer receipt", "is required"))?;
        self.validate_for(
            &writer.migration_id,
            &writer.source_project_id,
            &writer.source_snapshot_digest,
            writer.target_epoch,
        )
    }

    pub fn validate_for(
        &self,
        migration_id: &str,
        source_project_id: &str,
        source_snapshot_digest: &str,
        target_epoch: u64,
    ) -> Result<(), StateError> {
        validate_migration_identifier("migration id", migration_id)?;
        validate_migration_identifier("source project id", source_project_id)?;
        validate_migration_identifier("source snapshot digest", source_snapshot_digest)?;
        if target_epoch == 0 {
            return Err(StateError::invalid(
                "migration target epoch",
                "must be non-zero",
            ));
        }
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| StateError::invalid("migration writer receipt", "is required"))?;
        writer.validate()?;
        validate_migration_context(
            "writer",
            &writer.migration_id,
            &writer.source_project_id,
            &writer.source_snapshot_digest,
            writer.target_epoch,
            migration_id,
            source_project_id,
            source_snapshot_digest,
            target_epoch,
        )?;

        if self.identity_rebinds.len() != self.runtime_rebinds.len() {
            return Err(StateError::Invariant(format!(
                "migration identity/runtime receipt count mismatch: {} != {}",
                self.identity_rebinds.len(),
                self.runtime_rebinds.len()
            )));
        }

        let mut operations = BTreeSet::new();
        if !operations.insert(writer.operation_id.as_str().to_owned()) {
            return Err(StateError::Invariant(format!(
                "migration operation {} is recorded more than once",
                writer.operation_id
            )));
        }

        for receipt in &self.identity_rebinds {
            receipt.validate()?;
            validate_migration_context(
                "identity rebind",
                &receipt.migration_id,
                &receipt.source_project_id,
                &receipt.source_snapshot_digest,
                receipt.target_epoch,
                migration_id,
                source_project_id,
                source_snapshot_digest,
                target_epoch,
            )?;
            if receipt.fencing_token != writer.fencing_token {
                return Err(StateError::Invariant(format!(
                    "identity rebind {} uses a different fencing token",
                    receipt.operation_id
                )));
            }
            if receipt.source_epoch != writer.source_epoch {
                return Err(StateError::Invariant(format!(
                    "identity rebind {} uses a different source epoch",
                    receipt.operation_id
                )));
            }
            if !operations.insert(receipt.operation_id.as_str().to_owned()) {
                return Err(StateError::Invariant(format!(
                    "migration operation {} is recorded more than once",
                    receipt.operation_id
                )));
            }
        }

        for receipt in &self.runtime_rebinds {
            receipt.validate()?;
            validate_migration_context(
                "runtime rebind",
                &receipt.migration_id,
                &receipt.source_project_id,
                &receipt.source_snapshot_digest,
                receipt.target_epoch,
                migration_id,
                source_project_id,
                source_snapshot_digest,
                target_epoch,
            )?;
            if receipt.fencing_token != writer.fencing_token {
                return Err(StateError::Invariant(format!(
                    "runtime rebind {} uses a different fencing token",
                    receipt.operation_id
                )));
            }
            if receipt.source_epoch != writer.source_epoch {
                return Err(StateError::Invariant(format!(
                    "runtime rebind {} uses a different source epoch",
                    receipt.operation_id
                )));
            }
            if !operations.insert(receipt.operation_id.as_str().to_owned()) {
                return Err(StateError::Invariant(format!(
                    "migration operation {} is recorded more than once",
                    receipt.operation_id
                )));
            }
        }

        for identity in &self.identity_rebinds {
            let matches = self.runtime_rebinds.iter().filter(|runtime| {
                runtime.project_scope == identity.project_scope
                    && runtime.app_scope_id == identity.app_scope_id
                    && runtime.agent_id == identity.agent_id
                    && runtime.runtime_id == identity.runtime_id
                    && runtime.binding_id == identity.binding_id
                    && runtime.endpoint_generation == identity.endpoint_generation
                    && runtime.fencing_token == identity.fencing_token
            });
            if matches.count() != 1 {
                return Err(StateError::Invariant(format!(
                    "identity rebind {} has no unique matching runtime receipt",
                    identity.operation_id
                )));
            }
        }
        for runtime in &self.runtime_rebinds {
            let matches = self.identity_rebinds.iter().filter(|identity| {
                identity.project_scope == runtime.project_scope
                    && identity.app_scope_id == runtime.app_scope_id
                    && identity.agent_id == runtime.agent_id
                    && identity.runtime_id == runtime.runtime_id
                    && identity.binding_id == runtime.binding_id
                    && identity.endpoint_generation == runtime.endpoint_generation
                    && identity.fencing_token == runtime.fencing_token
            });
            if matches.count() != 1 {
                return Err(StateError::Invariant(format!(
                    "runtime rebind {} has no unique matching identity receipt",
                    runtime.operation_id
                )));
            }
        }
        Ok(())
    }
}

/// One host-wide reducer state.  Project state is nested under a canonical
/// project-scope key; AppServer registrations are nested under each project.
/// Command IDs are host-wide so retries cannot be rebound across projects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlobalState {
    pub epoch: u64,
    pub sequence: u64,
    pub revision: u64,
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectState>,
    #[serde(default)]
    pub current_thread_routes: BTreeMap<(String, String), RuntimeBinding>,
    /// Read-only compatibility index for durable bindings that carry a native
    /// App Server thread but no session id.  These records predate the strict
    /// dual key, so replay must keep them resolvable instead of aborting the
    /// host journal.  A thread may legitimately appear under several binding
    /// ids (a disposable project ran once on the same peer thread), so the
    /// index stores one binding per `(app_scope, thread, binding_id)` and keeps
    /// only the highest generation for each.  The key order is app scope,
    /// native thread, then binding id.
    #[serde(default)]
    pub legacy_thread_routes: BTreeMap<(String, String, String), RuntimeBinding>,
    #[serde(default)]
    pub current_thread_route_tombstones: BTreeMap<String, RuntimeBindingTombstone>,
    #[serde(default)]
    pub command_receipts: BTreeMap<String, CommandReceipt>,
    #[serde(default)]
    pub migration_commit_evidence: BTreeMap<String, MigrationCommitEvidence>,
    #[serde(default)]
    pub ledger_scan_receipts: BTreeMap<String, LedgerScanReceipt>,
    /// Durable, immutable grant-replacement intents and completion receipts.
    /// Keyed by the outer operation id; the intent is written before the first
    /// grant effect so an unresolved start survives restart without replay.
    #[serde(default)]
    pub master_grant_replacement_intents: BTreeMap<String, MasterGrantReplacementIntent>,
    #[serde(default)]
    pub master_grant_replacement_receipts: BTreeMap<String, MasterGrantReplacementReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LedgerScanReceipt {
    pub scan_id: String,
    pub scanned_ms: i64,
    pub classified: u32,
    pub transitioned: u32,
    pub unchanged: u32,
    pub blocked: u32,
    pub mailbox_messages_unchanged: bool,
}

impl LedgerScanReceipt {
    pub fn validate(&self) -> Result<(), StateError> {
        if self.scan_id.trim().is_empty() || self.scan_id.len() > 128 {
            return Err(StateError::invalid(
                "ledger scan receipt scan_id",
                "must be a non-empty short identifier",
            ));
        }
        if self.scanned_ms < 0 {
            return Err(StateError::invalid(
                "ledger scan receipt scanned_ms",
                "must be non-negative",
            ));
        }
        Ok(())
    }
}
