use super::*;
use crate::identity::{self, RuntimeIdentity};
use crate::proto::{AppServerCandidate, IdentityContextRequest, IdentityFacts};
use sha2::{Digest, Sha256};

/// Error from identity reconciliation that carries the nested Register receipt
/// IDs once that owner transaction has committed. The outer projection uses
/// the IDs to retain committed phases instead of reporting a bare pre-commit
/// `failed` for a post-commit error.
#[derive(Debug)]
struct ReconcileError {
    nested: Option<(String, String)>,
    error: anyhow::Error,
}

impl ReconcileError {
    fn before_register(error: anyhow::Error) -> Self {
        Self {
            nested: None,
            error,
        }
    }
}

impl std::fmt::Display for ReconcileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:#}", self.error)
    }
}

impl From<anyhow::Error> for ReconcileError {
    fn from(error: anyhow::Error) -> Self {
        Self::before_register(error)
    }
}

include!("identity_context/operations.rs");
include!("identity_context/recovery.rs");

fn validate_facts(facts: &IdentityFacts) -> anyhow::Result<()> {
    for (name, value) in [
        ("session_id", facts.session_id.as_deref()),
        ("thread_id", facts.thread_id.as_deref()),
        ("endpoint", facts.endpoint.as_deref()),
        ("namespace", facts.namespace.as_deref()),
    ] {
        if let Some(value) = value {
            if value.trim().is_empty() {
                anyhow::bail!("IDENTITY_FACT_INVALID: {name} must not be empty");
            }
            if matches!(name, "session_id" | "thread_id") {
                identity::validate_id_for_protocol(value)?;
            } else if value.chars().any(char::is_control) {
                anyhow::bail!("IDENTITY_FACT_INVALID: {name} contains control characters");
            }
        }
    }
    if let Some(namespace) = facts.namespace.as_deref() {
        if !matches!(namespace, "codex_tui" | "codex_app") {
            anyhow::bail!("IDENTITY_FACT_INVALID: unsupported namespace {namespace}");
        }
    }
    if let Some(endpoint) = facts.endpoint.as_deref() {
        let path = endpoint
            .strip_prefix("unix://")
            .ok_or_else(|| anyhow::anyhow!("IDENTITY_FACT_INVALID: endpoint must be unix://"))?;
        if !Path::new(path).is_absolute() {
            anyhow::bail!("IDENTITY_FACT_INVALID: endpoint must contain an absolute socket path");
        }
    }
    Ok(())
}

fn identity_operation_intent_digest(
    context: &ProjectContext,
    request: &IdentityContextRequest,
) -> String {
    let normalized = json!({
        "project_scope": context.project_scope.as_str(),
        "app_scope_id": context.app_scope_id.as_str(),
        "target_identity": null,
        "action": request.action,
        "invocation": request.invocation,
        "facts": request.facts,
        "approval": request.approval,
        "grant_approval": request.grant_approval,
    });
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&normalized).unwrap_or_default());
    format!("sha256:{:x}", digest.finalize())
}

/// Build a `Resp` for a typed operation outcome.
///
/// `Resp.ok` owns transport/operation success and the flattened payload is the
/// single `result` object. An inner `ok` would collide with `Resp.ok` on the
/// flattened wire and deserialize as duplicate keys, so the typed outcome is
/// carried only under `result`. A typed incomplete result is `ok: false` so the
/// CLI exit/MCP `isError` projection can distinguish it from transport failure.
fn typed_operation_resp(result: crate::proto::ContextOperationResult) -> Resp {
    let ok = result.outcome == "completed";
    let data = serde_json::json!({
        "result": serde_json::to_value(result).unwrap_or(serde_json::Value::Null)
    });
    Resp {
        ok,
        error: None,
        data,
    }
}

fn operation_result(
    request: &IdentityContextRequest,
    projection: crate::proto::IdentityOperationProjection,
    ok: bool,
    snapshot: Option<serde_json::Value>,
) -> crate::proto::ContextOperationResult {
    let phase = projection
        .business_receipts
        .last()
        .cloned()
        .or_else(|| ok.then(|| "context_complete".to_owned()));
    crate::proto::ContextOperationResult {
        operation_id: projection.operation_id,
        invocation: request.invocation.clone(),
        action: request.action.clone(),
        phase,
        outcome: projection.outcome,
        committed_phases: projection.business_receipts,
        failed_phase: None,
        requires: crate::proto::ContextOperationRequires {
            kind: (!ok).then(|| "operation".to_owned()),
            fields: Vec::new(),
            sources: Default::default(),
            approval: None,
            repair_invocation: None,
        },
        snapshot,
        owner_readback: Default::default(),
        queried_operation: None,
    }
}

fn operation_refusal(
    request: &IdentityContextRequest,
    error: String,
    repair_invocation: &str,
) -> crate::proto::ContextOperationResult {
    let mut result = operation_result(
        request,
        crate::proto::IdentityOperationProjection {
            operation_id: request.operation_id.clone(),
            phase: crate::proto::IdentityOperationPhase::Refused,
            outcome: "denied".into(),
            committed_phases: Vec::new(),
            business_receipts: Vec::new(),
            nested_command_id: None,
            nested_operation_id: None,
        },
        false,
        None,
    );
    result.phase = None;
    result
        .owner_readback
        .insert("error".into(), serde_json::Value::String(error));
    result.requires.repair_invocation = Some(repair_invocation.to_owned());
    result
}

fn stale_approval_denial(
    request: &IdentityContextRequest,
    approval: &IdentityApproval,
    observed: Option<&crate::server::global_state::RuntimeBinding>,
) -> crate::proto::ContextOperationResult {
    let mut result = operation_refusal(
        request,
        "APPROVAL_STALE_CONFLICT".into(),
        "collab context --op <new-operation-id> --approve-identity '<fresh approval object>'",
    );
    result.requires.kind = Some("approval".into());
    result.requires.repair_invocation = Some(
        "collab context --approve-identity '<exact approval object from requires.approval>'".into(),
    );
    result.owner_readback.insert(
        "error".into(),
        serde_json::Value::String("APPROVAL_STALE_CONFLICT".into()),
    );
    result.owner_readback.insert(
        "expected_incumbent".into(),
        serde_json::json!({
            "binding_id": approval.expected_incumbent_binding_id,
            "endpoint_generation": approval.expected_incumbent_generation,
        }),
    );
    if let Some(observed) = observed {
        let mut fresh_approval = serde_json::json!({
            "decision": "approved",
            "target_identity": observed.agent_id.as_str(),
            "project_scope": observed.project_scope.as_str(),
            "app_scope_id": observed.app_scope_id.as_str(),
            "action": "replace_binding",
            "expected_incumbent": {
                "binding_id": observed.binding_id.as_str(),
                "endpoint_generation": observed.endpoint_generation,
            }
        });
        fresh_approval["intent_digest"] =
            serde_json::Value::String(approval_digest(&fresh_approval, &request.facts));
        result.requires.approval = Some(fresh_approval);
        result.owner_readback.insert(
            "observed_incumbent".into(),
            serde_json::json!({
                "binding_id": observed.binding_id.as_str(),
                "endpoint_generation": observed.endpoint_generation,
                "target_identity": observed.agent_id.as_str(),
                "project_scope": observed.project_scope.as_str(),
                "app_scope_id": observed.app_scope_id.as_str(),
            }),
        );
    }
    result
}

fn missing_facts_result(
    request: &IdentityContextRequest,
    data: &serde_json::Value,
) -> crate::proto::ContextOperationResult {
    let update = data
        .get("snapshot")
        .and_then(|snapshot| snapshot.get("requires_identity_update"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let fields: Vec<String> = update
        .get("required_fields")
        .and_then(serde_json::Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let sources = fields
        .iter()
        .map(|field| {
            (
                field.clone(),
                update
                    .get("field_descriptions")
                    .and_then(|value| value.get(field))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
    crate::proto::ContextOperationResult {
        operation_id: request.operation_id.clone(),
        invocation: request.invocation.clone(),
        action: request.action.clone(),
        phase: None,
        outcome: "missing_facts".into(),
        committed_phases: Vec::new(),
        failed_phase: None,
        requires: crate::proto::ContextOperationRequires {
            kind: Some("identity_facts".into()),
            fields,
            sources,
            approval: None,
            repair_invocation: update
                .get("action")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
        },
        snapshot: None,
        owner_readback: Default::default(),
        queried_operation: None,
    }
}

/// Which facts the caller must still supply before an identity can be resolved.
///
/// Only the App Server path has fields the caller may have to add: its
/// endpoint, namespace, session and thread are four separate observations that
/// can arrive incomplete. The tmux and dsh paths each present one designed
/// anchor that is complete on its own, so they require nothing further.
///
/// A tmux or dsh anchor only short-circuits the request while no App Server
/// endpoint was observed. If an endpoint *is* present, the four App Server
/// facts are still demanded: the endpoint selects the App Server candidate
/// below, which reads all four fields unconditionally.
fn required_fields(facts: &IdentityFacts) -> Vec<&'static str> {
    if (facts.tmux.is_some() || facts.dsh_session_id.is_some()) && facts.endpoint.is_none() {
        return Vec::new();
    }
    [
        ("session_id", &facts.session_id),
        ("thread_id", &facts.thread_id),
        ("endpoint", &facts.endpoint),
        ("namespace", &facts.namespace),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.is_none().then_some(name))
    .collect()
}

enum PreflightIdentityError {
    MissingFacts(serde_json::Value),
    Error(anyhow::Error),
}

/// A user approval parsed from the exact staged schema. It binds one target
/// identity, one route scope, one action, and one exact incumbent fence. The
/// daemon validates these fields; it never trusts a caller's identity claim.
#[derive(Debug, Clone)]
struct IdentityApproval {
    decision: String,
    decided_by: String,
    target_identity: String,
    project_scope: String,
    app_scope_id: String,
    action: String,
    expected_incumbent_binding_id: String,
    expected_incumbent_generation: u64,
    intent_digest: String,
    approved_at_ms: i64,
}

/// The separate owner fence for a master grant replacement. It is never
/// inferred from the identity approval.
#[derive(Debug, Clone)]
struct GrantApproval {
    decision: String,
    decided_by: String,
    target_identity: String,
    project_scope: String,
    app_scope_id: String,
    action: String,
    expected_grant_id: String,
    expected_grant_generation: u64,
    intent_digest: String,
    approved_at_ms: i64,
}

/// One admitted approved-recovery intent: the exact target identity the daemon
/// will Register, the validated incumbent fence, and the optional separate
/// grant fence.
struct ApprovedAdmission {
    identity_approval: IdentityApproval,
    grant_approval: Option<GrantApproval>,
    grant_before: Option<crate::server::global_state::MasterGrant>,
    identity: identity::Identity,
}

impl ApprovedAdmission {
    /// The daemon-issued proof that lets the Register owner admit an approved
    /// stale-credential replacement for exactly this target and scope.
    fn register_proof(&self, context: &ProjectContext) -> RegisterApprovalProof {
        RegisterApprovalProof {
            target_identity: self.identity.worker_id.clone(),
            project_scope: context.project_scope.as_str().to_owned(),
            app_scope_id: context.app_scope_id.as_str().to_owned(),
            incumbent_binding_id: self.identity_approval.expected_incumbent_binding_id.clone(),
            incumbent_endpoint_generation: self.identity_approval.expected_incumbent_generation,
        }
    }

    /// A grant approval is a distinct owner authorization: the identity owner
    /// must not treat it as a second identity approval, and the grant owner
    /// commits its replacement in its own transaction.
    fn requires_grant_replacement(&self) -> bool {
        self.grant_approval.is_some()
    }
}

enum ApprovedAdmissionError {
    /// A typed refusal returned to the caller without any side effect.
    Denied(crate::proto::ContextOperationResult),
    /// A malformed request or an internal invariant break.
    Error(String),
}

fn validate_operation_request(request: &IdentityContextRequest) -> Result<(), String> {
    // A malformed query shape (query flag or query invocation/action reaching
    // the mutation owner) is reported before the unsupported-invocation gate so
    // callers get the precise selector error instead of a generic refusal.
    if request.query {
        return Err("IDENTITY_OPERATION_QUERY_SHAPE_INVALID".into());
    }
    if request.invocation == "query" || request.action == "query" {
        return Err("IDENTITY_OPERATION_QUERY_SHAPE_INVALID".into());
    }
    if request.action != "context" {
        return Err("IDENTITY_OPERATION_INTENT_INVALID: action must be context".into());
    }
    let approved = request.approval.is_some() || request.grant_approval.is_some();
    if approved {
        if request.invocation != "approved_recovery" {
            return Err(
                "IDENTITY_OPERATION_INTENT_INVALID: approvals require the approved_recovery invocation"
                    .into(),
            );
        }
    } else {
        if request.invocation != "automatic" && request.invocation != "supplement" {
            return Err("IDENTITY_CONTEXT_UNSUPPORTED: this adapter slice supports automatic and supplement context only".into());
        }
    }
    if request.query_capability.trim().is_empty() {
        return Err(
            "IDENTITY_OPERATION_CAPABILITY_REQUIRED: mutating context requires a query capability"
                .into(),
        );
    }
    Ok(())
}

fn parse_identity_approval(value: &serde_json::Value) -> Result<IdentityApproval, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "approval must be a JSON object".to_string())?;
    let decision = object
        .get("decision")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if decision != "approved" {
        return Err(format!(
            "IDENTITY_APPROVAL_NOT_APPROVED: decision must be approved, observed {decision}"
        ));
    }
    let target_identity = required_approval_string(object, "target_identity")?;
    let project_scope = required_approval_string(object, "project_scope")?;
    let app_scope_id = required_approval_string(object, "app_scope_id")?;
    let action = required_approval_string(object, "action")?;
    if action != "replace_binding" && action != "restore_identity" {
        return Err(format!(
            "IDENTITY_APPROVAL_ACTION_INVALID: action must be replace_binding or restore_identity, observed {action}"
        ));
    }
    let incumbent = object
        .get("expected_incumbent")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            "IDENTITY_APPROVAL_FENCE_REQUIRED: expected_incumbent is required".to_string()
        })?;
    let expected_incumbent_binding_id = required_approval_string(incumbent, "binding_id")?;
    let expected_incumbent_generation = incumbent
        .get("endpoint_generation")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            "IDENTITY_APPROVAL_FENCE_REQUIRED: expected_incumbent.endpoint_generation is required"
                .to_string()
        })?;
    Ok(IdentityApproval {
        decision: decision.to_owned(),
        decided_by: required_approval_string(object, "decided_by")?,
        target_identity,
        project_scope,
        app_scope_id,
        action,
        expected_incumbent_binding_id,
        expected_incumbent_generation,
        intent_digest: required_approval_string(object, "intent_digest")?,
        approved_at_ms: object
            .get("approved_at_ms")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                "IDENTITY_APPROVAL_METADATA_INVALID: approved_at_ms is required".to_string()
            })?,
    })
}

fn parse_grant_approval(value: &serde_json::Value) -> Result<GrantApproval, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "grant_approval must be a JSON object".to_string())?;
    let decision = object
        .get("decision")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if decision != "approved" {
        return Err(format!(
            "GRANT_APPROVAL_NOT_APPROVED: decision must be approved, observed {decision}"
        ));
    }
    let target_identity = required_approval_string(object, "target_identity")?;
    let project_scope = required_approval_string(object, "project_scope")?;
    let app_scope_id = required_approval_string(object, "app_scope_id")?;
    let action = required_approval_string(object, "action")?;
    if action != "replace_master_grant" {
        return Err(format!(
            "GRANT_APPROVAL_ACTION_INVALID: action must be replace_master_grant, observed {action}"
        ));
    }
    let grant = object
        .get("expected_grant")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "GRANT_APPROVAL_FENCE_REQUIRED: expected_grant is required".to_string())?;
    let expected_grant_id = required_approval_string(grant, "grant_id")?;
    let expected_grant_generation = grant
        .get("generation")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            "GRANT_APPROVAL_FENCE_REQUIRED: expected_grant.generation is required".to_string()
        })?;
    Ok(GrantApproval {
        decision: decision.to_owned(),
        decided_by: required_approval_string(object, "decided_by")?,
        target_identity,
        project_scope,
        app_scope_id,
        action,
        expected_grant_id,
        expected_grant_generation,
        intent_digest: required_approval_string(object, "intent_digest")?,
        approved_at_ms: object
            .get("approved_at_ms")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                "GRANT_APPROVAL_METADATA_INVALID: approved_at_ms is required".to_string()
            })?,
    })
}

fn approval_digest(approval: &serde_json::Value, facts: &IdentityFacts) -> String {
    let mut approval = approval.clone();
    if let Some(object) = approval.as_object_mut() {
        object.remove("intent_digest");
        object.remove("decided_by");
        object.remove("approved_at_ms");
    }
    let normalized = json!({"approval": approval, "facts": facts});
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&normalized).unwrap_or_default());
    format!("sha256:{:x}", digest.finalize())
}

/// The exact runtime binding fence the Register owner just committed, read
/// from the owner's own identity receipt. Both the grant owner and the
/// identity-only retention check bind to this value, never to a guessed one.
fn registered_binding_fence(response: &Resp) -> Option<(BindingId, u64)> {
    let runtime = response
        .data
        .get("identity_receipt")
        .and_then(|value| value.get("runtime"))?;
    let binding_id = runtime
        .get("binding_id")
        .and_then(serde_json::Value::as_str)?;
    let generation = runtime
        .get("endpoint_generation")
        .and_then(serde_json::Value::as_u64)?;
    Some((BindingId::new(binding_id.to_owned()).ok()?, generation))
}

/// The durable approval text recorded on a separately approved replacement
/// grant. It carries the user's decision and approval time so the authority
/// ledger keeps the same authorization evidence the identity owner validated.
/// Ordered durable business receipts proved by the current owner state after a
/// successful reconciliation. The list stops at the first owner whose durable
/// result is missing, so a receipt is only present when its own owner persisted
/// the effect. It never includes `context_complete`; the caller appends that
/// terminal receipt only after the optional authority receipt and the lease
/// proof are both present.
fn business_receipts_for_success(
    host: &Server,
    context: &ProjectContext,
    approved: Option<&ApprovedAdmission>,
    response: &Resp,
    nested: Option<&(String, String)>,
) -> Vec<String> {
    let mut receipts = Vec::new();
    if approved.is_some() {
        receipts.push("approval_decision".to_owned());
    }
    let Some((command_id, operation_id)) = nested else {
        return receipts;
    };
    if command_id.trim().is_empty() || operation_id.trim().is_empty() {
        return receipts;
    }
    receipts.push("nested_register".to_owned());
    let Some(worker_id) = response
        .data
        .get("identity_receipt")
        .and_then(|receipt| receipt.get("worker_id"))
        .and_then(serde_json::Value::as_str)
    else {
        return receipts;
    };
    let Some((binding_id, generation)) = registered_binding_fence(response) else {
        return receipts;
    };
    let route_scope = crate::scope::RouteScope {
        app_scope_id: context.app_scope_id.clone(),
        project_scope_id: context.project_scope.clone(),
    };
    // Route owner: the committed route must name exactly the binding version
    // the Register receipt returned, for this exact scope and principal.
    let route_proven = {
        let state = host.state.lock().unwrap();
        state
            .global
            .lookup_binding_for(&route_scope, &binding_id)
            .is_some_and(|binding| {
                binding.agent_id.as_str() == worker_id && binding.endpoint_generation == generation
            })
    };
    if !route_proven {
        return receipts;
    }
    receipts.push("route".to_owned());
    // Credential owner: the durable persisted identity, not the in-memory
    // worker record, must carry the exact committed binding version.
    let credential_proven = identity::read_persisted(&host.host_paths, worker_id)
        .ok()
        .flatten()
        .and_then(|identity| identity.runtime)
        .is_some_and(|runtime| {
            runtime.binding_id.as_str() == binding_id.as_str()
                && runtime.endpoint_generation == generation
        });
    if !credential_proven {
        return receipts;
    }
    receipts.push("credential".to_owned());
    // Lease owner: the Register owner armed a default direct-message lease for
    // this worker. It must still be live, not merely recorded.
    let lease_proven = {
        let state = host.state.lock().unwrap();
        state
            .notification_subscriptions
            .get(&crate::server::mailbox::default_direct_message_id(
                worker_id,
            ))
            .is_some_and(|subscription| {
                subscription.worker_id == worker_id
                    && subscription.status == "armed"
                    && subscription.expires_ms > crate::server::state::now_ms()
            })
    };
    if lease_proven {
        receipts.push("lease".to_owned());
    }
    receipts
}

/// Receipts that remain proved when reconciliation cannot complete. Only the
/// identity admission decision and a committed nested Register receipt are
/// durable at this point; route, credential, lease and `context_complete` are
/// never fabricated from a lifecycle name.
fn business_receipts_for_failure(
    approved: Option<&ApprovedAdmission>,
    nested: Option<&(String, String)>,
) -> Vec<String> {
    let mut receipts = Vec::new();
    if approved.is_some() {
        receipts.push("approval_decision".to_owned());
    }
    if let Some((command_id, operation_id)) = nested {
        if !command_id.trim().is_empty() && !operation_id.trim().is_empty() {
            receipts.push("nested_register".to_owned());
        }
    }
    receipts
}

fn grant_replacement_approval(approval: &GrantApproval) -> String {
    format!(
        "{}:{}:{}",
        approval.decision, approval.decided_by, approval.approved_at_ms
    )
}

fn required_approval_string(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<String, String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            format!("IDENTITY_APPROVAL_FIELD_REQUIRED: {field} must be a non-empty string")
        })
}

#[cfg(test)]
#[path = "identity_context/required_fields_tests.rs"]
mod required_fields_tests;
