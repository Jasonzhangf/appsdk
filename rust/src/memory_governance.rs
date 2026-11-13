use super::{MemoryError, RawCommitReceipt};
use crate::platform::{try_lock_exclusive, LockAttemptError};
use pipeline_runtime::governance::{
    bind_operation, CommitOwner, GovernanceError, GovernanceSnapshot, Invocation, OperationRequest,
    Subject,
};
use pipeline_runtime::{
    parse_graph_json, Cancellation, Identity, Operator, OperatorContext, Registry, ValueType,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

const CATEGORIES: [&str; 4] = ["plan", "path", "knowledge", "lesson"];
const ONTOLOGY: &str = include_str!("../../contracts/memory/rule-promotion.ontology.json");
const GRAPH: &str =
    include_str!("../../docs/dagpipe/graphs/project-memory-rule-promotion.graph.json");

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TargetScope {
    Project { root: String },
    Global { home: String },
}

#[derive(Clone, Debug)]
pub(super) struct ResolvedScope {
    pub(super) root: PathBuf,
    pub(super) global: bool,
    pub(super) target: TargetScope,
    raw_base: PathBuf,
    lock_path: PathBuf,
}

impl ResolvedScope {
    pub(super) fn target_string(&self) -> Result<String, MemoryError> {
        serde_json::to_string(&self.target).map_err(|error| {
            MemoryError::new(
                "MEMORY_SCOPE_INVALID",
                error.to_string(),
                "use a losslessly serializable project or global memory scope",
            )
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct ReviewRequest {
    pub(super) status: String,
    pub(super) level: i64,
    pub(super) evidence: Vec<String>,
}

#[derive(Clone, Debug)]
pub(super) struct MemoryChange {
    pub(super) patch: Value,
    pub(super) review: Option<ReviewRequest>,
    pub(super) allow_approval: bool,
    pub(super) force_rule_operation: bool,
}

pub(super) fn emit_error(error: MemoryError) -> ! {
    let value = error.to_json();
    if error.committed.is_empty() {
        eprintln!("{}", serde_json::to_string_pretty(&value).unwrap());
    } else {
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
    }
    std::process::exit(1);
}

struct ScopeLock {
    _file: File,
}

struct OwnerState {
    _lock: ScopeLock,
    receipts: Vec<RawCommitReceipt>,
    entries: Option<Vec<Value>>,
}

pub(super) struct MemoryCommitOwner {
    scope: ResolvedScope,
    state: Mutex<OwnerState>,
}

impl MemoryCommitOwner {
    fn new(scope: ResolvedScope) -> Result<Self, MemoryError> {
        let lock = lock_scope(&scope)?;
        Ok(Self {
            scope,
            state: Mutex::new(OwnerState {
                _lock: lock,
                receipts: Vec::new(),
                entries: None,
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, OwnerState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl CommitOwner for MemoryCommitOwner {
    fn apply(
        &self,
        invocation: &Invocation,
        prepared: Value,
        validate: &mut dyn FnMut(
            &GovernanceSnapshot,
            &GovernanceSnapshot,
        ) -> Result<(), GovernanceError>,
    ) -> Result<Value, GovernanceError> {
        let subject = invocation.subject();
        let patch = prepared.get("patch").ok_or_else(|| {
            GovernanceError::new("MEMORY_RULE_REQUEST_INVALID", "patch is required")
        })?;
        let current_entries = read_effective_entries(&self.scope)
            .map_err(|error| GovernanceError::new("MEMORY_SOURCE_INVALID", error.message))?;
        let current = current_entries
            .iter()
            .find(|entry| entry_id(entry) == subject.id)
            .ok_or_else(|| {
                GovernanceError::new(
                    "MEMORY_ENTRY_NOT_FOUND",
                    "create the rule candidate before promoting it",
                )
            })?;
        let candidate = build_candidate(
            Some(current),
            patch,
            prepared
                .get("review")
                .filter(|review| !review.is_null())
                .cloned(),
            &self.scope,
            true,
            false,
        )
        .map_err(governance_from_memory)?;
        ensure_category_preserved(current, &candidate).map_err(governance_from_memory)?;
        if rule_state(current) != "rule_approved" && !promotion_review_valid(prepared.get("review"))
        {
            return Err(GovernanceError::new(
                "MEMORY_PROMOTION_REVIEW_REQUIRED",
                "rule promotion requires level 1 or 2 with non-empty review evidence",
            ));
        }
        let current_snapshot = snapshot_for(&self.scope, current)?;
        let candidate_snapshot = snapshot_for(&self.scope, &candidate)?;
        validate(&current_snapshot, &candidate_snapshot)?;

        if equivalent_entry(current, &candidate) {
            let mut state = self.state();
            state.entries = Some(current_entries);
            return Ok(json!({
                "status": "deduplicated",
                "id": subject.id,
            }));
        }

        let category = category(candidate.get("category")).map_err(governance_from_memory)?;
        append_raw(&self.scope, category, &candidate).map_err(governance_from_memory)?;
        let revision = candidate
            .get("rule_control")
            .and_then(|control| control.get("revision"))
            .and_then(Value::as_u64);
        let receipt = RawCommitReceipt {
            scope: self.scope.target_string().map_err(governance_from_memory)?,
            id: subject.id.clone(),
            category: category.to_string(),
            revision,
        };
        self.state().receipts.push(receipt);
        let entries = read_effective_entries(&self.scope).map_err(governance_from_memory)?;
        self.state().entries = Some(entries);
        Ok(json!({
            "status": "committed",
            "id": subject.id,
            "category": category,
            "revision": revision,
        }))
    }
}

fn governance_from_memory(error: MemoryError) -> GovernanceError {
    GovernanceError::new(
        "MEMORY_GOVERNANCE_FAILED",
        format!("{}: {}", error.code, error.message),
    )
}

pub(super) fn submit_change(
    root: &Path,
    global: bool,
    patch: Value,
    review: Option<ReviewRequest>,
    allow_approval: bool,
    force_rule_operation: bool,
) -> Result<Value, MemoryError> {
    submit_batch(
        root,
        global,
        vec![MemoryChange {
            patch,
            review,
            allow_approval,
            force_rule_operation,
        }],
    )
}

pub(super) fn submit_batch(
    root: &Path,
    global: bool,
    changes: Vec<MemoryChange>,
) -> Result<Value, MemoryError> {
    let scope = resolve_scope(root, global)?;
    let owner = Arc::new(MemoryCommitOwner::new(scope.clone())?);
    let mut current_entries = read_effective_entries(&scope)?;
    let mut preflight_entries = current_entries.clone();
    preflight_changes(&scope, &mut preflight_entries, &changes)?;
    let change_ids = changes
        .iter()
        .map(change_id)
        .collect::<Result<Vec<_>, _>>()?;
    let mut results = Vec::new();

    for (change_index, change) in changes.into_iter().enumerate() {
        let id = change
            .patch
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                change
                    .patch
                    .get("memory_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .ok_or_else(|| {
                MemoryError::new(
                    "MEMORY_ENTRY_ID_REQUIRED",
                    "declare id or memory_id",
                    "provide a stable memory ID",
                )
            })?;
        if !valid_id(&id) {
            return Err(MemoryError::new(
                "MEMORY_ID_INVALID",
                "invalid memory ID",
                "use an alphanumeric, dash, underscore, dot, or slash ID",
            ));
        }
        let current = current_entries
            .iter()
            .find(|entry| entry_id(entry) == id)
            .cloned();
        let has_approval = change
            .patch
            .get("rule_control")
            .and_then(|control| control.get("approval"))
            .and_then(|approval| approval.get("state"))
            .and_then(Value::as_str)
            == Some("approved");
        let should_operate = change.force_rule_operation
            || (change.allow_approval && has_approval && current.as_ref().is_some_and(is_rule));

        if should_operate {
            let current = current.clone().ok_or_else(|| {
                MemoryError::new(
                    "MEMORY_ENTRY_NOT_FOUND",
                    "create the rule candidate before promoting it",
                    "save the rule candidate with entry --input first",
                )
            })?;
            let behavior = if rule_state(&current) == "rule_approved" {
                "memory.update_approved_rule"
            } else {
                "memory.promote_rule"
            };
            let operation = bind_memory_operation(owner.clone(), behavior)?;
            let request = OperationRequest {
                subject: Subject {
                    object_type: "memory.entry".into(),
                    scope: scope.target_string()?,
                    id: id.clone(),
                },
                event: if behavior == "memory.promote_rule" {
                    "promote_rule".into()
                } else {
                    "update_approved_rule".into()
                },
                data: json!({"patch": change.patch, "review": change.review.as_ref().map(|review| json!({
                    "status": review.status,
                    "level": review.level,
                    "evidence": review.evidence,
                }))}),
            };
            let identity = operation_identity();
            let result = operation.call(request, identity, &Cancellation::default());
            if let Err(error) = result {
                let error = MemoryError::new(
                    "MEMORY_OPERATION_FAILED",
                    format!("{error}"),
                    "repair the rule request or approval target",
                );
                let receipts = owner.state().receipts.clone();
                return Err(with_committed_prefix(
                    error,
                    &receipts,
                    &change_ids[change_index..],
                    &scope,
                ));
            }
            current_entries = owner.state().entries.clone().ok_or_else(|| {
                MemoryError::new(
                    "MEMORY_COMMIT_MISSING",
                    "operation completed without committed entries",
                    "inspect raw JSONL before retrying",
                )
            })?;
            results.push(json!({
                "status": "operation_committed",
                "id": id,
                "category": current_entries
                    .iter()
                    .find(|entry| entry_id(entry) == id)
                    .and_then(|entry| entry.get("category").cloned())
                    .unwrap_or_else(|| json!("knowledge")),
            }));
            continue;
        }

        let review = change
            .review
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|error| {
                MemoryError::new(
                    "MEMORY_REVIEW_INVALID",
                    error.to_string(),
                    "repair the review input",
                )
            })?;
        let candidate = build_candidate(
            current.as_ref(),
            &change.patch,
            review,
            &scope,
            change.allow_approval,
            true,
        )?;
        let category = category(candidate.get("category"))?;
        let semantically_unchanged = current
            .as_ref()
            .is_some_and(|current| equivalent_entry(current, &candidate));
        if semantically_unchanged {
            results.push(json!({
                "accepted": true,
                "deduplicated": true,
                "id": id,
                "category": category,
            }));
            continue;
        }

        append_raw(&scope, category, &candidate).map_err(|error| {
            let receipts = owner.state().receipts.clone();
            with_committed_prefix(error, &receipts, &change_ids[change_index..], &scope)
        })?;
        let receipt = RawCommitReceipt {
            scope: scope.target_string()?,
            id: id.clone(),
            category: category.to_string(),
            revision: candidate
                .get("rule_control")
                .and_then(|control| control.get("revision"))
                .and_then(Value::as_u64),
        };
        owner.state().receipts.push(receipt.clone());
        current_entries = read_effective_entries(&scope).map_err(|error| {
            let receipts = owner.state().receipts.clone();
            with_committed_prefix(error, &receipts, &change_ids[change_index + 1..], &scope)
        })?;
        results.push(json!({
            "accepted": true,
            "id": id,
            "category": category,
            "revision": receipt.revision,
            "approval_not_applied": if has_approval
                && current.is_none()
                && rule_state(&candidate) == "rule_candidate"
            {
                json!("new_entry_requires_promotion")
            } else {
                Value::Null
            },
        }));
    }

    let receipts = owner.state().receipts.clone();
    project_batch(&scope, &receipts, &current_entries, results)
}

fn preflight_changes(
    scope: &ResolvedScope,
    entries: &mut Vec<Value>,
    changes: &[MemoryChange],
) -> Result<(), MemoryError> {
    for change in changes {
        let id = change_id(change)?;
        let current = entries.iter().find(|entry| entry_id(entry) == id).cloned();
        let review = review_value(change)?;
        let promotion_review_ready = promotion_review_valid(review.as_ref());
        let candidate = build_candidate(
            current.as_ref(),
            &change.patch,
            review,
            scope,
            change.allow_approval,
            true,
        )?;
        if let Some(current) = current.as_ref() {
            ensure_category_preserved(current, &candidate)?;
        }
        if should_use_operation(change, current.as_ref()) {
            if !current
                .as_ref()
                .is_some_and(|current| rule_state(current) == "rule_approved")
                && !promotion_review_ready
            {
                return Err(MemoryError::new(
                    "MEMORY_PROMOTION_REVIEW_REQUIRED",
                    "rule promotion requires level 1 or 2 with non-empty review evidence",
                    "provide reviewed level 1 or 2 with non-empty evidence",
                ));
            }
            let relation_ok = rule_state(&candidate) == "rule_approved";
            let valid = user_approval_valid(&candidate)
                && control_preservation_valid(current.as_ref(), &candidate)
                && rule_contract_valid(&candidate)
                && approval_target_valid(&scope.target_string()?, &id, &candidate, relation_ok);
            if !valid {
                return Err(MemoryError::new(
                    "MEMORY_OPERATION_FAILED",
                    "MEMORY_GOVERNANCE_FAILED: rule operation preflight rejected the candidate",
                    "repair the rule request or approval target",
                ));
            }
        }
        if let Some(position) = entries.iter().position(|entry| entry_id(entry) == id) {
            entries[position] = candidate;
        } else {
            entries.push(candidate);
        }
    }
    Ok(())
}

fn ensure_category_preserved(current: &Value, candidate: &Value) -> Result<(), MemoryError> {
    if category(current.get("category"))? != category(candidate.get("category"))? {
        return Err(MemoryError::new(
            "MEMORY_CATEGORY_CHANGE",
            "keep one category per memory ID or create a new ID",
            "keep one category per memory ID or create a new ID",
        ));
    }
    Ok(())
}

fn promotion_review_valid(review: Option<&Value>) -> bool {
    let Some(review) = review else {
        return false;
    };
    let status = review.get("status").and_then(Value::as_str);
    let level = review.get("level").and_then(Value::as_i64);
    let evidence = review
        .get("evidence")
        .and_then(Value::as_array)
        .is_some_and(|evidence| !evidence.is_empty() && evidence.iter().all(Value::is_string));
    status == Some("reviewed") && matches!(level, Some(1 | 2)) && evidence
}

fn with_committed_prefix(
    error: MemoryError,
    receipts: &[RawCommitReceipt],
    uncommitted_ids: &[String],
    scope: &ResolvedScope,
) -> MemoryError {
    if receipts.is_empty() {
        error
    } else {
        MemoryError::committed_prefix(
            error.code,
            error.message,
            error.next,
            receipts.to_vec(),
            uncommitted_ids.to_vec(),
            recovery(scope),
        )
    }
}

fn change_id(change: &MemoryChange) -> Result<String, MemoryError> {
    let id = change
        .patch
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            change
                .patch
                .get("memory_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .ok_or_else(|| {
            MemoryError::new(
                "MEMORY_ENTRY_ID_REQUIRED",
                "declare id or memory_id",
                "provide a stable memory ID",
            )
        })?;
    if !valid_id(&id) {
        return Err(MemoryError::new(
            "MEMORY_ID_INVALID",
            "invalid memory ID",
            "use an alphanumeric, dash, underscore, dot, or slash ID",
        ));
    }
    Ok(id)
}

fn review_value(change: &MemoryChange) -> Result<Option<Value>, MemoryError> {
    change
        .review
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| {
            MemoryError::new(
                "MEMORY_REVIEW_INVALID",
                error.to_string(),
                "repair the review input",
            )
        })
}

fn has_approved_input(patch: &Value) -> bool {
    patch
        .get("rule_control")
        .and_then(|control| control.get("approval"))
        .and_then(|approval| approval.get("state"))
        .and_then(Value::as_str)
        == Some("approved")
}

fn should_use_operation(change: &MemoryChange, current: Option<&Value>) -> bool {
    change.force_rule_operation
        || (change.allow_approval
            && has_approved_input(&change.patch)
            && current.is_some_and(is_rule))
}

fn project_batch(
    scope: &ResolvedScope,
    receipts: &[RawCommitReceipt],
    entries: &[Value],
    results: Vec<Value>,
) -> Result<Value, MemoryError> {
    let Some(receipt) = receipts.first().cloned() else {
        return Ok(json!({
            "accepted": true,
            "committed": false,
            "deduplicated": true,
            "updates": results,
        }));
    };
    match super::sync_index_from_entries(&scope.root, scope.global, entries) {
        Ok(index) => {
            let mut value = json!({
            "accepted": true,
            "committed": true,
            "projection_updated": true,
                "id": receipt.id,
                "scope": receipt.scope,
                "category": receipt.category,
                "revision": receipt.revision,
                "index": index,
            });
            if receipts.len() == 1 {
                if let Some(entry) = entries.iter().find(|entry| entry_id(entry) == receipt.id) {
                    value["memory_level"] = json!(super::memory_level(entry));
                    value["review_status"] = json!(super::review_status(entry));
                    value["detail_path"] = json!(super::detail_display_path(
                        scope.global,
                        &receipt.id,
                        super::memory_level(entry)
                    ));
                }
                if let Some(result) = results.into_iter().next() {
                    if let Some(approval_not_applied) = result.get("approval_not_applied") {
                        if !approval_not_applied.is_null() {
                            value["approval_not_applied"] = approval_not_applied.clone();
                        }
                    }
                }
            } else {
                value["updates"] = Value::Array(results);
                value["receipts"] = Value::Array(
                    receipts
                        .iter()
                        .map(|receipt| {
                            json!({
                                "id": receipt.id,
                                "scope": receipt.scope,
                                "category": receipt.category,
                                "revision": receipt.revision,
                            })
                        })
                        .collect(),
                );
            }
            Ok(value)
        }
        Err(error) => Err(MemoryError::committed_prefix(
            error.code,
            error.message,
            error.next,
            receipts.to_vec(),
            Vec::new(),
            recovery(scope),
        )),
    }
}

fn recovery(scope: &ResolvedScope) -> Vec<String> {
    let mut command = vec!["project-memory".to_string(), "index".to_string()];
    if scope.global {
        command.push(scope.root.to_string_lossy().to_string());
        command.push("--global".into());
    } else {
        command.push(scope.root.to_string_lossy().to_string());
    }
    command.push("--rebuild-only".into());
    command
}

fn bind_memory_operation(
    owner: Arc<MemoryCommitOwner>,
    behavior: &str,
) -> Result<pipeline_runtime::governance::CompiledOperation, MemoryError> {
    let ontology: Value = serde_json::from_str(ONTOLOGY).map_err(|error| {
        MemoryError::new(
            "MEMORY_ONTOLOGY_INVALID",
            error.to_string(),
            "repair contracts/memory/rule-promotion.ontology.json",
        )
    })?;
    let graph = parse_graph_json(GRAPH).map_err(|error| {
        MemoryError::new(
            "MEMORY_GRAPH_INVALID",
            error.to_string(),
            "repair docs/dagpipe/graphs/project-memory-rule-promotion.graph.json",
        )
    })?;
    let mut registry = Registry::default();
    registry.register(MemoryPrepare).map_err(operator_error)?;
    registry
        .register(MemoryEntryContract)
        .map_err(operator_error)?;
    registry
        .register(MemoryUserApproval)
        .map_err(operator_error)?;
    registry
        .register(MemoryControlPreservation)
        .map_err(operator_error)?;
    registry
        .register(MemoryRuleContract)
        .map_err(operator_error)?;
    registry
        .register(MemoryApprovalTarget)
        .map_err(operator_error)?;
    let capabilities = BTreeSet::from(["ontology.commit".to_string()]);
    bind_operation(&ontology, behavior, graph, registry, owner, &capabilities).map_err(|error| {
        MemoryError::new(
            "MEMORY_OPERATION_BIND_FAILED",
            error.to_string(),
            "repair the memory ontology, graph, or operator registry",
        )
    })
}

fn operator_error(error: pipeline_runtime::CompileError) -> MemoryError {
    MemoryError::new(
        "MEMORY_OPERATOR_REGISTRATION_FAILED",
        error.to_string(),
        "repair the Memory governance operator registry",
    )
}

fn operation_identity() -> Identity {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Identity {
        project_id: "project-memory".into(),
        graph_id: "project-memory-rule-promotion".into(),
        graph_version: "1".into(),
        execution_id: format!("memory-{nonce}"),
        attempt_id: format!("memory-{nonce}-1"),
    }
}

fn resolve_scope(root: &Path, global: bool) -> Result<ResolvedScope, MemoryError> {
    let (canonical, target, raw_base, lock_path) = if global {
        let home = super::home_dir();
        let canonical = canonicalize_or_tail(&home)?;
        let home = path_string(&canonical)?;
        (
            canonical.clone(),
            TargetScope::Global { home },
            canonical.join("global"),
            canonical.join("global/.write.lock"),
        )
    } else {
        let canonical = canonicalize_or_tail(root)?;
        let root = path_string(&canonical)?;
        (
            canonical.clone(),
            TargetScope::Project { root },
            canonical.join("memory"),
            canonical.join("memory/.write.lock"),
        )
    };
    Ok(ResolvedScope {
        root: canonical,
        global,
        target,
        raw_base,
        lock_path,
    })
}

fn path_string(path: &Path) -> Result<String, MemoryError> {
    path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
        MemoryError::new(
            "MEMORY_SCOPE_INVALID",
            "memory scope path is not valid UTF-8",
            "use a losslessly serializable project or global memory scope",
        )
    })
}

fn canonicalize_or_tail(path: &Path) -> Result<PathBuf, MemoryError> {
    if let Ok(value) = fs::canonicalize(path) {
        return Ok(value);
    }
    if !path.is_absolute() {
        return Err(MemoryError::new(
            "MEMORY_SCOPE_INVALID",
            "memory scope must be an absolute path",
            "provide an existing project root or absolute memory home",
        ));
    }
    let mut ancestor = path.to_path_buf();
    let mut tail = Vec::new();
    while !ancestor.exists() {
        let name = ancestor.file_name().ok_or_else(|| {
            MemoryError::new(
                "MEMORY_SCOPE_INVALID",
                "memory scope has no existing ancestor",
                "provide an existing project root or memory home",
            )
        })?;
        tail.push(name.to_os_string());
        ancestor.pop();
    }
    let mut value = fs::canonicalize(&ancestor).map_err(|error| {
        MemoryError::new(
            "MEMORY_SCOPE_INVALID",
            error.to_string(),
            "repair the memory scope path",
        )
    })?;
    for component in tail.iter().rev() {
        value.push(component);
    }
    Ok(value)
}

fn lock_scope(scope: &ResolvedScope) -> Result<ScopeLock, MemoryError> {
    if let Some(parent) = scope.lock_path.parent() {
        if fs::symlink_metadata(parent)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(MemoryError::new(
                "MEMORY_SOURCE_SYMLINK",
                "memory directory is a symlink",
                "replace it with a project-owned directory",
            ));
        }
        fs::create_dir_all(parent).map_err(|error| {
            MemoryError::new(
                "MEMORY_WRITE_FAILED",
                error.to_string(),
                "repair memory permissions",
            )
        })?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&scope.lock_path)
        .map_err(|error| {
            MemoryError::new(
                "MEMORY_WRITE_FAILED",
                error.to_string(),
                "repair memory permissions",
            )
        })?;
    match try_lock_exclusive(&file) {
        Ok(()) => Ok(ScopeLock { _file: file }),
        Err(LockAttemptError::WouldBlock) => Err(MemoryError::new(
            "MEMORY_WRITE_BUSY",
            "another Memory writer holds the scope lock",
            "retry after the current writer finishes",
        )),
        Err(LockAttemptError::Io(error)) => Err(MemoryError::new(
            "MEMORY_WRITE_FAILED",
            error.to_string(),
            "repair memory permissions",
        )),
    }
}

fn read_raw_entries(scope: &ResolvedScope) -> Result<Vec<Value>, MemoryError> {
    if fs::symlink_metadata(&scope.raw_base)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(MemoryError::new(
            "MEMORY_SOURCE_SYMLINK",
            "memory directory is a symlink",
            "replace it with a project-owned directory",
        ));
    }
    let mut entries = Vec::new();
    for category in CATEGORIES {
        let path = scope.raw_base.join(format!("{category}.jsonl"));
        if fs::symlink_metadata(&path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(MemoryError::new(
                "MEMORY_SOURCE_SYMLINK",
                "memory source is a symlink",
                "replace it with a project-owned regular file",
            ));
        }
        if !path.is_file() {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|error| {
            MemoryError::new(
                "MEMORY_SOURCE_INVALID",
                error.to_string(),
                "repair the memory JSONL source",
            )
        })?;
        for (line, value) in text.lines().enumerate() {
            if value.trim().is_empty() {
                continue;
            }
            let value = serde_json::from_str(value).map_err(|error| {
                MemoryError::new(
                    "MEMORY_SOURCE_INVALID",
                    format!("line {}: {error}", line + 1),
                    "repair the memory JSONL source",
                )
            })?;
            entries.push(value);
        }
    }
    Ok(entries)
}

fn read_effective_entries(scope: &ResolvedScope) -> Result<Vec<Value>, MemoryError> {
    let raw = read_raw_entries(scope)?;
    Ok(super::effective_entries(raw))
}

fn append_raw(scope: &ResolvedScope, category: &str, entry: &Value) -> Result<(), MemoryError> {
    let path = scope.raw_base.join(format!("{category}.jsonl"));
    let mut text = if path.is_file() {
        fs::read_to_string(&path).map_err(|error| {
            MemoryError::new(
                "MEMORY_SOURCE_INVALID",
                error.to_string(),
                "repair the memory JSONL source",
            )
        })?
    } else {
        String::new()
    };
    text.push_str(&serde_json::to_string(entry).map_err(|error| {
        MemoryError::new(
            "MEMORY_SOURCE_INVALID",
            error.to_string(),
            "repair the memory entry",
        )
    })?);
    text.push('\n');
    super::atomic_write_result(&path, &text)
}

fn build_candidate(
    current: Option<&Value>,
    patch: &Value,
    review: Option<Value>,
    scope: &ResolvedScope,
    allow_approval: bool,
    strip_new_approval: bool,
) -> Result<Value, MemoryError> {
    let patch = patch.as_object().ok_or_else(|| {
        MemoryError::new(
            "MEMORY_ENTRY_INVALID",
            "entry input must be a JSON object",
            "provide a structured entry object",
        )
    })?;
    let mut candidate = current.cloned().unwrap_or_else(|| json!({}));
    let mut incoming_rule_control = None;
    for (key, value) in patch {
        if key == "rule_control" {
            incoming_rule_control = Some(value.clone());
        } else {
            candidate[key] = value.clone();
        }
    }

    let current_is_rule = current.is_some_and(is_rule);
    let entry_kind = candidate
        .get("entry_kind")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    if current_is_rule && entry_kind.as_deref() == Some("fact") {
        return Err(MemoryError::new(
            "MEMORY_RULE_CONTROL_INVALID",
            "an existing rule cannot be changed into a fact",
            "create a separate fact instead of deleting rule control",
        ));
    }
    if entry_kind
        .as_deref()
        .is_some_and(|kind| kind != "fact" && kind != "rule")
    {
        return Err(MemoryError::new(
            "MEMORY_ENTRY_KIND_INVALID",
            "entry_kind must be fact or rule",
            "repair the structured entry",
        ));
    }
    let candidate_is_rule = entry_kind.as_deref() == Some("rule") || current_is_rule;

    if candidate_is_rule {
        candidate["entry_kind"] = json!("rule");
        let control = match incoming_rule_control.clone() {
            Some(value) => {
                let incoming = value.as_object().ok_or_else(|| {
                    MemoryError::new(
                        "MEMORY_RULE_CONTROL_INVALID",
                        "rule_control must be an object",
                        "repair the structured entry",
                    )
                })?;
                let mut control = current
                    .and_then(|entry| entry.get("rule_control"))
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                for (key, value) in incoming {
                    control.insert(key.clone(), value.clone());
                }
                Value::Object(control)
            }
            None => current
                .and_then(|entry| entry.get("rule_control"))
                .cloned()
                .ok_or_else(|| {
                    MemoryError::new(
                        "MEMORY_RULE_CONTROL_INVALID",
                        "a rule update must not delete rule_control",
                        "omit rule_control to preserve current control",
                    )
                })?,
        };
        if !control.is_object() {
            return Err(MemoryError::new(
                "MEMORY_RULE_CONTROL_INVALID",
                "rule_control must be an object",
                "repair the structured entry",
            ));
        }
        candidate["rule_control"] = control;
    } else {
        if candidate
            .get("rule_control")
            .is_some_and(|control| !control.is_null())
        {
            return Err(MemoryError::new(
                "MEMORY_RULE_CONTROL_INVALID",
                "a fact cannot carry rule_control",
                "remove rule_control or mark the entry as rule",
            ));
        }
        let candidate = candidate.as_object_mut().unwrap();
        candidate.remove("entry_kind");
        candidate.remove("rule_control");
    }

    if candidate_is_rule {
        let current_control = current.and_then(|entry| entry.get("rule_control"));
        let candidate_title = candidate.get("title").cloned();
        let candidate_content = candidate.get("content").cloned();
        let candidate_clause = candidate
            .get("rule_control")
            .and_then(|control| control.get("clause"))
            .cloned();
        let semantic_changed = current
            .map(|current| {
                current.get("title") != candidate_title.as_ref()
                    || current.get("content") != candidate_content.as_ref()
                    || current_control.and_then(|control| control.get("clause"))
                        != candidate_clause.as_ref()
            })
            .unwrap_or(true);
        let candidate_control = candidate
            .get_mut("rule_control")
            .and_then(Value::as_object_mut)
            .expect("rule control object");
        let supplied_revision = incoming_rule_control
            .as_ref()
            .and_then(|control| control.get("revision"))
            .and_then(Value::as_u64);
        let supplied_approval = incoming_rule_control
            .as_ref()
            .and_then(|control| control.get("approval"))
            .cloned();
        let current_revision = current_control
            .and_then(|control| control.get("revision"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let computed_revision = if current_is_rule {
            if semantic_changed {
                current_revision.saturating_add(1).max(1)
            } else {
                current_revision.max(1)
            }
        } else {
            1
        };
        if let Some(supplied) = supplied_revision {
            if supplied != computed_revision {
                if semantic_changed
                    && supplied_approval
                        .as_ref()
                        .and_then(|approval| approval.get("state"))
                        .and_then(Value::as_str)
                        == Some("approved")
                    && current.is_some_and(|entry| rule_state(entry) == "rule_approved")
                {
                    return Err(MemoryError::new(
                        "MEMORY_RULE_REAPPROVAL_REQUIRED",
                        "a changed approved rule needs a new matching approval",
                        "provide a new approval for the computed rule revision",
                    ));
                }
                return Err(MemoryError::new(
                    "MEMORY_RULE_REVISION_MISMATCH",
                    format!(
                        "supplied revision {supplied} does not match computed revision {computed_revision}"
                    ),
                    "omit revision and let Memory compute it",
                ));
            }
        }
        candidate_control.insert("revision".into(), json!(computed_revision));

        let current_approval = current_control
            .and_then(|control| control.get("approval"))
            .cloned();
        let approved_input = supplied_approval
            .as_ref()
            .and_then(|approval| approval.get("state"))
            .and_then(Value::as_str)
            == Some("approved");
        let current_approved = current.is_some_and(|entry| rule_state(entry) == "rule_approved");
        if current_approved
            && ((semantic_changed && (!allow_approval || !approved_input))
                || (!semantic_changed && supplied_approval.is_some() && !approved_input))
        {
            return Err(MemoryError::new(
                "MEMORY_RULE_REAPPROVAL_REQUIRED",
                "a changed approved rule needs a new matching approval",
                "provide a new approval in a promotion or explicit rule update",
            ));
        }
        let force_candidate =
            !allow_approval || (strip_new_approval && !current_is_rule && approved_input);
        if force_candidate {
            if current_approved {
                candidate_control["approval"] =
                    current_approval.unwrap_or(json!({"state":"candidate"}));
            } else {
                candidate_control.insert("approval".into(), json!({"state":"candidate"}));
            }
        } else if let Some(approval) = supplied_approval {
            candidate_control.insert("approval".into(), approval);
        } else if current_approval.is_some() && !semantic_changed {
            candidate_control["approval"] = current_approval.unwrap();
        } else {
            candidate_control.insert("approval".into(), json!({"state":"candidate"}));
        }
        validate_approval_scope(candidate_control.get("approval"), &scope.target)?;
    }

    let content = super::entry_text(&candidate);
    candidate["content"] = Value::String(content);
    candidate["category"] = Value::String(category(candidate.get("category"))?.to_string());
    candidate["tags"] = Value::Array(
        merged_string_array(
            current.and_then(|entry| entry.get("tags")),
            candidate.get("tags"),
        )
        .into_iter()
        .map(Value::String)
        .collect(),
    );
    candidate["source_refs"] = Value::Array(
        merged_string_array(
            current.and_then(|entry| entry.get("source_refs")),
            candidate.get("source_refs"),
        )
        .into_iter()
        .map(Value::String)
        .collect(),
    );
    candidate["title"] = candidate
        .get("title")
        .cloned()
        .unwrap_or_else(|| json!(entry_id(&candidate)));
    if let Some(review) = review {
        let level = review.get("level").and_then(Value::as_i64).unwrap_or(3);
        let evidence: Vec<String> = review
            .get("evidence")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let reviewed = review.get("status").and_then(Value::as_str) == Some("reviewed")
            && !evidence.is_empty();
        if candidate_is_rule && rule_state(&candidate) != "rule_approved" {
            candidate["memory_level"] = json!(3);
        } else {
            candidate["memory_level"] = json!(if reviewed { level } else { 3 });
        }
        candidate["review_status"] = json!(if reviewed { "reviewed" } else { "unreviewed" });
        candidate["review_evidence"] = json!(evidence);
    } else {
        let level = candidate
            .get("memory_level")
            .and_then(Value::as_i64)
            .or_else(|| current.map(super::memory_level))
            .unwrap_or(3);
        candidate["memory_level"] = json!(if candidate_is_rule
            && rule_state(&candidate) != "rule_approved"
        {
            3
        } else {
            level
        });
        if candidate.get("review_status").is_none() {
            candidate["review_status"] = json!("unreviewed");
        }
        if candidate.get("review_evidence").is_none() {
            candidate["review_evidence"] = json!([]);
        }
    }
    if candidate.get("created_at").is_none() {
        candidate["created_at"] = json!(super::now());
    }
    candidate["updated_at"] = json!(super::now());
    candidate["layer"] = json!(super::memory_level(&candidate));
    candidate["detail_path"] = json!(super::detail_display_path(
        scope.global,
        &entry_id(&candidate),
        super::memory_level(&candidate)
    ));
    Ok(candidate)
}

fn validate_approval_scope(
    approval: Option<&Value>,
    expected: &TargetScope,
) -> Result<(), MemoryError> {
    let Some(approval) = approval else {
        return Ok(());
    };
    if approval.get("state").and_then(Value::as_str) != Some("approved") {
        return Ok(());
    }
    let scope: TargetScope =
        serde_json::from_value(approval.get("scope").cloned().unwrap_or_else(|| json!({})))
            .map_err(|error| {
                MemoryError::new(
                    "MEMORY_APPROVAL_SCOPE_INVALID",
                    error.to_string(),
                    "provide a canonical project or global approval scope",
                )
            })?;
    if &scope != expected {
        return Err(MemoryError::new(
            "MEMORY_APPROVAL_SCOPE_MISMATCH",
            "approval scope does not match the selected Memory scope",
            "provide an approval for this exact scope",
        ));
    }
    Ok(())
}

fn merged_string_array(previous: Option<&Value>, current: Option<&Value>) -> Vec<String> {
    let mut values = Vec::new();
    for source in [previous, current] {
        if let Some(array) = source.and_then(Value::as_array) {
            for value in array.iter().filter_map(Value::as_str) {
                if !values.iter().any(|existing| existing == value) {
                    values.push(value.to_string());
                }
            }
        }
    }
    values.sort();
    values
}

fn equivalent_entry(current: &Value, candidate: &Value) -> bool {
    [
        "content",
        "title",
        "category",
        "entry_kind",
        "rule_control",
        "tags",
        "source_refs",
        "memory_level",
        "review_status",
        "review_evidence",
        "importance",
    ]
    .iter()
    .all(|key| {
        current.get(*key).unwrap_or(&Value::Null) == candidate.get(*key).unwrap_or(&Value::Null)
    })
}

fn snapshot_for(
    scope: &ResolvedScope,
    entry: &Value,
) -> Result<GovernanceSnapshot, GovernanceError> {
    let id = entry_id(entry);
    let target = scope.target_string().map_err(governance_from_memory)?;
    let state = rule_state(entry);
    let objects = vec![json!({
        "type": "memory.entry",
        "scope": target,
        "id": id,
        "state": state,
        "data": entry,
    })];
    let relations = if state == "rule_approved" {
        vec![json!({
            "type": "memory.approval_target",
            "from": {"type":"memory.entry","scope":target,"id":id},
            "to": {"type":"memory.entry","scope":target,"id":id},
        })]
    } else {
        Vec::new()
    };
    Ok(GovernanceSnapshot { objects, relations })
}

fn category(value: Option<&Value>) -> Result<&str, MemoryError> {
    let value = value.and_then(Value::as_str).unwrap_or("knowledge");
    if CATEGORIES.contains(&value) {
        Ok(value)
    } else {
        Err(MemoryError::new(
            "MEMORY_CATEGORY_INVALID",
            "use plan, path, knowledge, or lesson",
            "repair the entry category",
        ))
    }
}

fn is_rule(value: &Value) -> bool {
    value.get("entry_kind").and_then(Value::as_str) == Some("rule")
        || value
            .get("rule_control")
            .is_some_and(|control| !control.is_null())
}

fn rule_state(value: &Value) -> &'static str {
    if !is_rule(value) {
        return "record";
    }
    if value
        .get("rule_control")
        .and_then(|control| control.get("approval"))
        .and_then(|approval| approval.get("state"))
        .and_then(Value::as_str)
        == Some("approved")
    {
        "rule_approved"
    } else {
        "rule_candidate"
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
}

fn entry_id(value: &Value) -> String {
    value
        .get("id")
        .or_else(|| value.get("memory_id"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

struct MemoryPrepare;
impl Operator for MemoryPrepare {
    fn name(&self) -> &'static str {
        "memory.prepare_rule"
    }
    fn version(&self) -> &'static str {
        "1"
    }
    fn input_type(&self) -> ValueType {
        ValueType::Object
    }
    fn output_type(&self) -> ValueType {
        ValueType::Object
    }
    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        input
            .as_object()
            .cloned()
            .map(Value::Object)
            .ok_or_else(|| "memory.prepare_rule requires a structured request object".into())
    }
}

struct MemoryEntryContract;
impl Operator for MemoryEntryContract {
    fn name(&self) -> &'static str {
        "memory.entry_contract"
    }
    fn version(&self) -> &'static str {
        "1"
    }
    fn input_type(&self) -> ValueType {
        ValueType::Object
    }
    fn output_type(&self) -> ValueType {
        ValueType::Boolean
    }
    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        let state = input
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let data = input.get("data").and_then(Value::as_object);
        let valid = match state {
            "record" => data.is_some_and(|data| data.get("rule_control").is_none()),
            "rule_candidate" => data.is_some_and(|data| {
                data.get("rule_control")
                    .and_then(Value::as_object)
                    .is_some_and(|control| {
                        control
                            .get("revision")
                            .and_then(Value::as_u64)
                            .is_some_and(|revision| revision >= 1)
                            && control.get("clause").is_some()
                    })
            }),
            "rule_approved" => data.is_some_and(|data| {
                data.get("rule_control")
                    .and_then(Value::as_object)
                    .is_some_and(|control| {
                        control
                            .get("revision")
                            .and_then(Value::as_u64)
                            .is_some_and(|revision| revision >= 1)
                            && control.get("clause").is_some()
                            && control
                                .get("approval")
                                .and_then(|approval| approval.get("state"))
                                .and_then(Value::as_str)
                                == Some("approved")
                    })
            }),
            _ => false,
        };
        Ok(Value::Bool(valid))
    }
}

struct MemoryUserApproval;
impl Operator for MemoryUserApproval {
    fn name(&self) -> &'static str {
        "memory.user_approval"
    }
    fn version(&self) -> &'static str {
        "1"
    }
    fn input_type(&self) -> ValueType {
        ValueType::Object
    }
    fn output_type(&self) -> ValueType {
        ValueType::Boolean
    }
    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        Ok(Value::Bool(
            candidate_data(&input).is_some_and(user_approval_valid),
        ))
    }
}

struct MemoryControlPreservation;
impl Operator for MemoryControlPreservation {
    fn name(&self) -> &'static str {
        "memory.control_preservation"
    }
    fn version(&self) -> &'static str {
        "1"
    }
    fn input_type(&self) -> ValueType {
        ValueType::Object
    }
    fn output_type(&self) -> ValueType {
        ValueType::Boolean
    }
    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        Ok(Value::Bool(candidate_data(&input).is_some_and(
            |candidate| control_preservation_valid(snapshot_data(&input, "current"), candidate),
        )))
    }
}

struct MemoryRuleContract;
impl Operator for MemoryRuleContract {
    fn name(&self) -> &'static str {
        "memory.rule_contract"
    }
    fn version(&self) -> &'static str {
        "1"
    }
    fn input_type(&self) -> ValueType {
        ValueType::Object
    }
    fn output_type(&self) -> ValueType {
        ValueType::Boolean
    }
    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        Ok(Value::Bool(
            candidate_data(&input).is_some_and(rule_contract_valid),
        ))
    }
}

struct MemoryApprovalTarget;
impl Operator for MemoryApprovalTarget {
    fn name(&self) -> &'static str {
        "memory.approval_target"
    }
    fn version(&self) -> &'static str {
        "1"
    }
    fn input_type(&self) -> ValueType {
        ValueType::Object
    }
    fn output_type(&self) -> ValueType {
        ValueType::Boolean
    }
    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        let subject = input.get("subject");
        let relation_ok = input
            .get("candidate")
            .and_then(|value| value.get("relations"))
            .and_then(Value::as_array)
            .is_some_and(|relations| {
                relations
                    .iter()
                    .filter(|relation| {
                        relation.get("type").and_then(Value::as_str)
                            == Some("memory.approval_target")
                    })
                    .count()
                    == 1
            });
        let valid = subject.is_some_and(|subject| {
            let subject_scope = subject.get("scope").and_then(Value::as_str);
            let subject_id = subject.get("id").and_then(Value::as_str);
            subject_scope.is_some_and(|scope| {
                subject_id.is_some_and(|id| {
                    candidate_data(&input).is_some_and(|candidate| {
                        approval_target_valid(scope, id, candidate, relation_ok)
                    })
                })
            })
        });
        Ok(Value::Bool(valid))
    }
}

fn snapshot_data<'a>(input: &'a Value, name: &str) -> Option<&'a Value> {
    input
        .get(name)
        .and_then(|value| value.get("objects"))
        .and_then(Value::as_array)
        .and_then(|objects| objects.first())
        .and_then(|object| object.get("data"))
}

fn candidate_data(input: &Value) -> Option<&Value> {
    snapshot_data(input, "candidate")
}

fn user_approval_valid(candidate: &Value) -> bool {
    let approval = candidate
        .get("rule_control")
        .and_then(|control| control.get("approval"));
    approval.is_some_and(|approval| {
        approval.get("state").and_then(Value::as_str) == Some("approved")
            && approval.get("role").and_then(Value::as_str) == Some("user")
            && approval
                .get("source")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
            && approval
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
    })
}

fn control_preservation_valid(current: Option<&Value>, candidate: &Value) -> bool {
    current
        .and_then(|entry| entry.get("rule_control"))
        .is_none_or(Value::is_null)
        || candidate
            .get("rule_control")
            .is_some_and(|control| !control.is_null())
}

fn rule_contract_valid(candidate: &Value) -> bool {
    candidate
        .get("rule_control")
        .and_then(|control| control.get("clause"))
        .is_some_and(full_clause)
}

fn approval_target_valid(
    subject_scope: &str,
    subject_id: &str,
    candidate: &Value,
    relation_ok: bool,
) -> bool {
    let approval = candidate
        .get("rule_control")
        .and_then(|control| control.get("approval"));
    let subject_scope = serde_json::from_str::<TargetScope>(subject_scope).ok();
    let approval_scope = approval
        .and_then(|approval| approval.get("scope"))
        .and_then(|scope| serde_json::from_value::<TargetScope>(scope.clone()).ok());
    relation_ok
        && subject_scope.is_some()
        && subject_scope == approval_scope
        && approval.is_some_and(|approval| {
            approval.get("state").and_then(Value::as_str) == Some("approved")
                && approval.get("entry_id").and_then(Value::as_str) == Some(subject_id)
                && approval.get("revision")
                    == candidate
                        .get("rule_control")
                        .and_then(|control| control.get("revision"))
        })
}

fn full_clause(clause: &Value) -> bool {
    let Some(clause) = clause.as_object() else {
        return false;
    };
    let strings = [
        "clause_id",
        "axis",
        "axis_id",
        "scope",
        "scenario",
        "trigger",
        "state_before",
        "state_after",
        "owner",
    ];
    let arrays = [
        "allowed_actions",
        "forbidden_actions",
        "effects",
        "evidence",
    ];
    strings.iter().all(|key| {
        clause
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    }) && arrays.iter().all(|key| {
        clause
            .get(*key)
            .and_then(Value::as_array)
            .is_some_and(|values| !values.is_empty() && values.iter().all(Value::is_string))
    }) && clause.get("governance").is_some_and(|governance| {
        let Some(governance) = governance.as_object() else {
            return false;
        };
        [
            "edge",
            "gate_kind",
            "check",
            "allow_case",
            "deny_case",
            "observation",
        ]
        .iter()
        .all(|key| {
            governance
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
        })
    })
}
