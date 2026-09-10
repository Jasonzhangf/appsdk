use std::path::Path;

use serde_json::Value;

use crate::long_horizon_policy::ExecutionRole;
use crate::{collab_master_status, run_goal_collab_command, GOAL_COLLAB_READ_TIMEOUT};

/// Resolve the execution role only from a verified Collab context and live
/// worker/master records. Any unavailable probe is surfaced and becomes
/// `Unknown`; there is no master fallback.
pub(crate) fn execution_role(root: &Path, status: &Option<Value>) -> ExecutionRole {
    let context = match collab_context(root) {
        Ok(context) => context,
        Err(error) => {
            eprintln!("{}", error);
            return ExecutionRole::Unknown;
        }
    };
    let Some(peer) = context["identity"]["worker_id"].as_str().map(str::to_owned) else {
        return ExecutionRole::Unknown;
    };

    let Some(status) = status.as_ref() else {
        return ExecutionRole::Unknown;
    };
    if let Some(subagents) = status["subagents"].as_array() {
        if subagents.iter().any(|sub| {
            sub["peer"].as_str() == Some(peer.as_str())
                && sub["status"].as_str().unwrap_or("") != "closed"
        }) {
            return ExecutionRole::ManagedSubagent;
        }
    }

    let Some(worker) = status["workers"].as_array().and_then(|workers| {
        workers
            .iter()
            .find(|worker| worker["id"].as_str() == Some(peer.as_str()))
    }) else {
        return ExecutionRole::Unknown;
    };
    let worker_is_live = worker["endpoint_live"].as_bool() == Some(true)
        && worker["identity_valid"].as_bool() == Some(true)
        && worker["suspected_offline"].as_bool() == Some(false);
    if !worker_is_live {
        return ExecutionRole::Unknown;
    }

    let master_matches_context = match collab_master_matches_context(root, &peer, &context) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{}", error);
            return ExecutionRole::Unknown;
        }
    };
    if master_matches_context == Some(true) {
        return ExecutionRole::Master;
    }

    match worker["role"].as_str() {
        Some("peer") | Some("worker") => ExecutionRole::Worker,
        _ if master_matches_context == Some(false) => ExecutionRole::Worker,
        _ => ExecutionRole::Unknown,
    }
}

fn collab_context(root: &Path) -> Result<Value, String> {
    let mut command = std::process::Command::new("collab");
    command.arg("context").current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_READ_TIMEOUT)
        .map_err(|error| format!("COLLAB_CONTEXT_UNAVAILABLE:{}", error))?;
    if !out.status.success() {
        return Err(format!(
            "COLLAB_CONTEXT_FAILED:exit={}",
            out.status.code().unwrap_or(1)
        ));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|error| format!("COLLAB_CONTEXT_JSON_INVALID:{}", error))
}

fn collab_master_matches_context(
    root: &Path,
    peer: &str,
    context: &Value,
) -> Result<Option<bool>, String> {
    let master_status = collab_master_status(root)?;
    let master = match master_status
        .get("master")
        .filter(|value| value.is_object())
    {
        Some(master) => master,
        None => return Ok(None),
    };
    let master_peer = match master["worker_id"]
        .as_str()
        .filter(|id| !id.trim().is_empty())
    {
        Some(peer) => peer,
        None => return Ok(None),
    };
    if master["endpoint_live"].as_bool() != Some(true) {
        return Ok(None);
    }
    if master_peer != peer {
        return Ok(Some(false));
    }
    let master_pane = match master["pane"]
        .as_str()
        .filter(|pane| !pane.trim().is_empty())
    {
        Some(pane) => pane,
        None => return Ok(None),
    };
    let context_pane = match context["identity"]["pane"]
        .as_str()
        .filter(|pane| !pane.trim().is_empty())
    {
        Some(pane) => pane,
        None => return Ok(None),
    };
    if master_pane != context_pane {
        return Ok(None);
    }
    Ok(Some(true))
}
