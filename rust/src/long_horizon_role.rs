use serde_json::Value;

use crate::long_horizon_policy::ExecutionRole;

/// `collab context` returns its runtime binding under `result.snapshot`. The
/// long-horizon role and owner gates share one normalized view so they never
/// read a second, independently drifted envelope.
pub(crate) fn normalized_collab_context(context: &Value) -> Value {
    if let Some(snapshot) = context
        .get("result")
        .and_then(|result| result.get("snapshot"))
        .filter(|snapshot| snapshot.is_object())
    {
        return snapshot.clone();
    }
    context.clone()
}

/// Context owns identity. Reachability observations do not revoke a peer's
/// orchestration role; project master authority still has its own owner.
pub(crate) fn execution_role(context: &Value) -> ExecutionRole {
    if context["identity"]["worker_id"]
        .as_str()
        .is_none_or(|id| id.trim().is_empty())
    {
        return ExecutionRole::Unknown;
    }

    if context["identity"]["role"].as_str() == Some("managed-subagent")
        || context["identity"]["kind"].as_str() == Some("managed-subagent")
        || context["role_brief"]["derivation"]["kind"].as_str() == Some("managed-subagent")
    {
        return ExecutionRole::ManagedSubagent;
    }
    match context["identity"]["role"]
        .as_str()
        .or_else(|| context["role_brief"]["role"].as_str())
    {
        Some("master") => ExecutionRole::Master,
        Some("managed-subagent") => ExecutionRole::ManagedSubagent,
        Some("peer" | "worker") => ExecutionRole::Worker,
        None if context["identity"]["kind"].as_str() == Some("peer") => ExecutionRole::Worker,
        _ => ExecutionRole::Unknown,
    }
}
