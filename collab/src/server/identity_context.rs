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

fn add_optional_master_choice(snapshot: &mut serde_json::Value) {
    let Some(holder) = snapshot["master"]["worker_id"].as_str() else {
        return;
    };
    if snapshot["identity"]["worker_id"].as_str() == Some(holder) {
        return;
    }
    if let Some(actions) = snapshot["next_actions"].as_array_mut() {
        actions.push(json!("Peer restoration succeeded. Optionally replace the project master with explicit user authorization: collab context --approve-grant '{\"action\":\"restore_master\",\"authorization\":\"<user authorization>\"}'"));
    }
}

fn validate_facts(facts: &IdentityFacts) -> anyhow::Result<()> {
    if facts.caller_pid == Some(0) {
        anyhow::bail!("IDENTITY_FACT_INVALID: caller_pid must be non-zero");
    }
    if let Some(hint) = facts.pane_hint.as_deref() {
        crate::client::adapters::tmux::validate_pane_id(hint)
            .map_err(|error| anyhow::anyhow!("IDENTITY_FACT_INVALID: {error}"))?;
    }
    // Native facts are not admission inputs when the current pane is selected.
    if facts.tmux.is_some() {
        return Ok(());
    }
    if let Some(candidate) = facts.dsh.as_ref() {
        if facts.desktop_bridge.is_some()
            || facts.session_id.is_some()
            || facts.thread_id.is_some()
            || facts.endpoint.is_some()
            || facts.namespace.is_some()
        {
            anyhow::bail!("IDENTITY_FACT_INVALID: DSH facts cannot be mixed with native AppServer or Desktop facts");
        }
        if facts
            .dsh_session_id
            .as_deref()
            .is_some_and(|session| session != candidate.session_id)
        {
            anyhow::bail!(
                "IDENTITY_FACT_CONFLICT: DSH candidate differs from the current DSH session"
            );
        }
        for (name, value) in [
            ("endpoint", &candidate.endpoint),
            ("runtime_id", &candidate.runtime_id),
            ("agent_id", &candidate.agent_id),
            ("session_id", &candidate.session_id),
            ("cwd", &candidate.cwd),
        ] {
            if value.trim().is_empty() || value.chars().any(char::is_control) {
                anyhow::bail!("IDENTITY_FACT_INVALID: dsh.{name} must be non-empty and contain no control characters");
            }
        }
        let path = candidate.endpoint.strip_prefix("unix://").ok_or_else(|| {
            anyhow::anyhow!("IDENTITY_FACT_INVALID: dsh.endpoint must be unix://")
        })?;
        if !Path::new(path).is_absolute() || !Path::new(&candidate.cwd).is_absolute() {
            anyhow::bail!("IDENTITY_FACT_INVALID: DSH endpoint and cwd must be absolute");
        }
        identity::validate_id_for_protocol(&candidate.session_id)?;
        return Ok(());
    }
    if let Some(candidate) = facts.desktop_bridge.as_ref() {
        if facts.session_id.as_deref() != Some(candidate.session_id.as_str())
            || facts.thread_id.as_deref() != Some(candidate.thread_id.as_str())
        {
            anyhow::bail!(
                "IDENTITY_FACT_INVALID: Desktop candidate must match current session and thread"
            );
        }
    }
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

/// Facts that define the durable intent. A caller process id and pane hint are
/// invocation-local observation inputs, so retries from another process keep
/// the same intent and approval digest.
fn digest_facts(facts: &IdentityFacts) -> IdentityFacts {
    let mut normalized = facts.clone();
    normalized.caller_pid = None;
    normalized.pane_hint = None;
    normalized
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
        "facts": digest_facts(&request.facts),
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
/// Every path needs a real agent session anchor. A tmux placement can locate
/// delivery, but it cannot prove which agent session owns the name or identity.
fn required_fields(facts: &IdentityFacts) -> Vec<&'static str> {
    let mut required = Vec::new();
    if facts.session_id.is_none() && facts.dsh_session_id.is_none() && facts.dsh.is_none() {
        required.push("session_id");
    }
    if facts.dsh.is_some()
        || facts.desktop_bridge.is_some()
        || facts.tmux.is_some()
        || (facts.dsh_session_id.is_some() && facts.endpoint.is_none())
    {
        return required;
    }
    required.extend(
        [
            ("thread_id", &facts.thread_id),
            ("endpoint", &facts.endpoint),
            ("namespace", &facts.namespace),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.is_none().then_some(name)),
    );
    required
}

enum PreflightIdentityError {
    MissingFacts(serde_json::Value),
    Error(anyhow::Error),
}

/// A user approval for one explicit identity recovery. The old full shape with
/// an incumbent fence remains accepted, while the minimal user intent only
/// needs a target, action, and authorization text; the daemon fills the
/// current incumbent fence inside its identity gate.
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
    authorization: String,
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
            || matches!(
                self.identity_approval.action.as_str(),
                "restore_master" | "restore_peer"
            )
    }
}

enum ApprovedAdmissionError {
    /// A malformed request or an internal invariant break.
    Error(String),
}

fn name_adjudication_control(
    request: &IdentityContextRequest,
) -> Result<Option<(String, super::name_state::NameRecoveryVerdict, String)>, ApprovedAdmissionError>
{
    let Some(raw) = request
        .grant_approval
        .as_ref()
        .or(request.approval.as_ref())
        .filter(|raw| raw["action"] == "adjudicate_name")
    else {
        return Ok(None);
    };
    let parse = || -> Result<_, String> {
        let object = raw.as_object().ok_or("NAME_ADJUDICATION_INVALID")?;
        let operation = required_approval_string(object, "operation_id")?;
        let verdict = serde_json::from_value(raw["verdict"].clone())
            .map_err(|_| "NAME_ADJUDICATION_VERDICT_INVALID".to_owned())?;
        let evidence = raw["evidence"]
            .as_str()
            .ok_or("NAME_ADJUDICATION_EVIDENCE_REQUIRED")?
            .to_owned();
        Ok((operation, verdict, evidence))
    };
    parse().map(Some).map_err(ApprovedAdmissionError::Error)
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
        if request.invocation != "automatic"
            && request.invocation != "supplement"
            && request.invocation != "manual_communication_restore"
        {
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
    let project_scope = optional_approval_string(object, "project_scope").unwrap_or_default();
    let app_scope_id = optional_approval_string(object, "app_scope_id").unwrap_or_default();
    let action =
        optional_approval_string(object, "action").unwrap_or_else(|| "restore_identity".into());
    if !matches!(
        action.as_str(),
        "replace_binding" | "restore_identity" | "restore_master" | "restore_peer"
    ) {
        return Err(format!(
            "IDENTITY_APPROVAL_ACTION_INVALID: action must be replace_binding, restore_identity, restore_master or restore_peer, observed {action}"
        ));
    }
    let (expected_incumbent_binding_id, expected_incumbent_generation) = match object
        .get("expected_incumbent")
    {
        Some(value) => {
            let incumbent = value.as_object().ok_or_else(|| {
                "IDENTITY_APPROVAL_FENCE_INVALID: expected_incumbent must be an object".to_string()
            })?;
            (
                    required_approval_string(incumbent, "binding_id")?,
                    incumbent
                        .get("endpoint_generation")
                        .and_then(serde_json::Value::as_u64)
                        .ok_or_else(|| {
                            "IDENTITY_APPROVAL_FENCE_INVALID: expected_incumbent.endpoint_generation is required"
                                .to_string()
                        })?,
                )
        }
        None => (String::new(), 0),
    };
    let authorization = ["authorization", "user_authorization", "approval"]
        .into_iter()
        .find_map(|field| optional_approval_string(object, field))
        .unwrap_or_default();
    // Full legacy approvals already carry the user's decision and provenance.
    // Only the simplified intent needs a separate authorization text.
    if authorization.trim().is_empty()
        && !(object.get("expected_incumbent").is_some()
            && optional_approval_string(object, "decided_by").is_some()
            && object
                .get("approved_at_ms")
                .and_then(serde_json::Value::as_i64)
                .is_some_and(|time| time > 0))
    {
        return Err(
            "IDENTITY_APPROVAL_AUTHORIZATION_REQUIRED: authorization text is required".into(),
        );
    }
    Ok(IdentityApproval {
        decision: decision.to_owned(),
        decided_by: optional_approval_string(object, "decided_by").unwrap_or_else(|| "user".into()),
        target_identity,
        project_scope,
        app_scope_id,
        action,
        expected_incumbent_binding_id,
        expected_incumbent_generation,
        intent_digest: optional_approval_string(object, "intent_digest").unwrap_or_default(),
        approved_at_ms: object
            .get("approved_at_ms")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        authorization,
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
    let normalized = json!({"approval": approval, "facts": digest_facts(facts)});
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
    runtime: &Server,
    context: &ProjectContext,
    approved: Option<&ApprovedAdmission>,
    response: &Resp,
    nested: Option<&(String, String)>,
) -> Vec<String> {
    let mut receipts = Vec::new();
    if approved.is_some() {
        receipts.push("approval_decision".to_owned());
    }
    if let Some((command_id, operation_id)) = nested {
        if command_id.trim().is_empty() || operation_id.trim().is_empty() {
            return receipts;
        }
        receipts.push("nested_register".to_owned());
    }
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
    let Some(app_scope_id) = response.data["identity_receipt"]["runtime"]["appserver_id"]
        .as_str()
        .and_then(|app| AppServerId::new(app.to_owned()).ok())
    else {
        return receipts;
    };
    let route_scope = crate::scope::RouteScope {
        app_scope_id,
        project_scope_id: context.project_scope.clone(),
    };
    // Route owner: the committed route must name exactly the binding version
    // the Register receipt returned, for this exact scope and principal.
    let committed_binding = {
        let state = runtime.state.lock().unwrap();
        state
            .global
            .lookup_binding_for(&route_scope, &binding_id)
            .filter(|binding| {
                binding.agent_id.as_str() == worker_id && binding.endpoint_generation == generation
            })
            .cloned()
    };
    let route_proven = committed_binding.as_ref().is_some_and(|binding| {
        let state = host.state.lock().unwrap();
        match binding.tmux_endpoint.as_ref() {
            Some(endpoint) => state.global.lookup_tmux_route(endpoint) == Some(binding),
            None => match (
                binding.session_id.as_ref(),
                binding.native_thread_id.as_ref(),
            ) {
                (Some(session), Some(thread)) => {
                    state.global.lookup_current_thread_route(session, thread) == Some(binding)
                }
                _ => false,
            },
        }
    });
    if !route_proven {
        return receipts;
    }
    receipts.push("route".to_owned());
    // Credential owner: the durable persisted identity, not the in-memory
    // worker record, must carry the exact committed binding version.
    let credential_proven = identity::read_persisted(&host.host_paths, worker_id)
        .ok()
        .flatten()
        .is_some_and(|identity| {
            let state = runtime.state.lock().unwrap();
            identity.worker_id == worker_id
                && identity.project_scope.as_ref() == Some(&context.project_scope)
                && state.workers.get(worker_id).is_some_and(|worker| {
                    worker.token == identity.token && worker.transport == identity.transport
                })
                && identity.runtime.as_ref().is_some_and(|saved| {
                    saved.binding_id == binding_id
                        && saved.endpoint_generation == generation
                        && committed_binding.as_ref().is_some_and(|binding| {
                            saved.agent_id == binding.agent_id
                                && saved.appserver_id == binding.app_scope_id
                                && saved.runtime_id == binding.runtime_id
                                && saved.session_id == binding.session_id
                                && saved.native_thread_id == binding.native_thread_id
                        })
                })
        });
    if !credential_proven {
        return receipts;
    }
    receipts.push("credential".to_owned());
    // Lease owner: the worker must have a live direct-message receive channel.
    // That can be the system-owned default lease or an explicit owner
    // re-subscription created after the default was explicitly unsubscribed.
    let lease_proven = {
        let state = runtime.state.lock().unwrap();
        let now = crate::server::state::now_ms();
        state
            .workers
            .get(worker_id)
            .and_then(selected_transport_for_worker)
            .is_some_and(|transport| {
                state
                    .notification_subscriptions
                    .values()
                    .any(|subscription| {
                        subscription.matches(worker_id, "direct-message", None, now)
                            && subscription_matches_transport(subscription, &transport)
                    })
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

fn optional_approval_string(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Option<String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
#[path = "identity_context/required_fields_tests.rs"]
mod required_fields_tests;
