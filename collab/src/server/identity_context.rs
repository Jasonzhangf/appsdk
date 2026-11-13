use super::*;
use crate::identity::{self, RuntimeIdentity};
use crate::proto::{AppServerCandidate, IdentityContextRequest, IdentityFacts};

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RexRecoveryBaseline {
    pub(crate) key: crate::server::binding_slot::IdentityBindingKey,
    pub(crate) generation: u64,
    pub(crate) current: crate::server::binding_slot::BindingValue,
}

#[derive(Clone)]
pub(crate) enum RexRecoveryAdmission {
    Verified(crate::server::binding_slot::VerifiedRexExecutionAssociation),
    Missing(String),
    CallerMismatch(String),
    DisplacedExecution(crate::server::binding_slot::VerifiedRexExecutionAssociation),
}

#[derive(Clone)]
pub(crate) struct RexRecoveryObservation {
    pub(crate) baseline: Option<RexRecoveryBaseline>,
    pub(crate) transport: crate::proto::SelectedTransport,
    pub(crate) admission: RexRecoveryAdmission,
}

include!("identity_context/mapped_context.rs");
include!("identity_context/recovery.rs");
include!("identity_context/dsh_rebind.rs");
include!("identity_context/desktop_lifecycle.rs");

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
    if let Some(candidate) = facts.rex.as_ref() {
        if facts.dsh.is_some()
            || facts.desktop_bridge.is_some()
            || facts.tmux.is_some()
            || facts.thread_id.is_some()
            || facts.endpoint.is_some()
            || facts.namespace.is_some()
        {
            anyhow::bail!(
                "IDENTITY_FACT_INVALID: Rex facts cannot be combined with another transport"
            );
        }
        for (field, value) in [
            ("server", &candidate.server),
            ("session_id", &candidate.session_id),
            ("block_id", &candidate.block_id),
            ("cwd", &candidate.cwd),
        ] {
            if value.trim().is_empty() || value.chars().any(char::is_control) {
                anyhow::bail!("IDENTITY_FACT_INVALID: rex.{field} must be non-empty without control characters");
            }
        }
        if let Some(session) = facts.session_id.as_ref() {
            identity::validate_id_for_protocol(session)?;
        }
        return Ok(());
    }
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
        || facts.rex.is_some()
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

#[cfg(test)]
#[path = "identity_context/required_fields_tests.rs"]
mod required_fields_tests;
