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

pub(crate) fn effective_peer_charter(context: &Value) -> Option<String> {
    if execution_role(context) != ExecutionRole::Worker {
        return None;
    }
    let brief = &context["role_brief"];
    let workflow = &brief["workflow_template"];
    if workflow["identity"]["source"] != "master_override"
        && !workflow["specialties"]
            .as_array()
            .is_some_and(|roles| !roles.is_empty())
    {
        return None;
    }
    let mut charter = brief["role_task"].as_str()?.to_owned();
    for duty in brief["responsibilities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        charter.push_str("\n- ");
        charter.push_str(duty);
    }
    for (field, label) in [
        ("next_action", "下一步"),
        ("completion_action", "完成交接"),
    ] {
        if let Some(action) = brief[field].as_str() {
            charter.push_str(&format!("\n\n{label}：{action}"));
        }
    }
    Some(charter)
}
