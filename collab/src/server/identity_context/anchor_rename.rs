pub(crate) const PHYSICAL_ANCHOR_PENDING: &str = "CONTROL_COMMITTED_PHYSICAL_ANCHOR_PENDING: ";

/// Optional physical effect result for one prepared anchor.
///
/// A reliable manifest target makes the physical rename part of this
/// operation, but it is not the identity commit. A real rename/readback error
/// stays an explicit incomplete result so the committed identity and basic
/// communication survive and the caller can retry the remaining effect.
pub(crate) enum PhysicalAnchorOutcome {
    /// The current label already matched the reliable target; no command ran.
    Consistent { target: String },
    /// No reliable target exists, so no physical effect was required.
    NotRequested,
    /// The new physical name was applied and read back.
    Applied { target: String },
    /// A reliable target exists but the actual rename/readback failed.
    Incomplete {
        target: String,
        observed: Option<String>,
        error: String,
    },
}

impl PhysicalAnchorOutcome {
    /// Observed physical readback for the operation outcome. `observed` is the
    /// caller's current physical label when known; it is never invented.
    pub(crate) fn readback(&self, observed: Option<String>) -> serde_json::Value {
        match self {
            Self::Consistent { target } => json!({
                "status": "complete", "target": target, "observed": target,
                "error": null, "renamed": false
            }),
            Self::NotRequested => json!({
                "status": "not_requested", "target": null, "observed": observed,
                "error": null, "renamed": false
            }),
            Self::Applied { target } => json!({
                "status": "complete", "target": target, "observed": target,
                "error": null, "renamed": true
            }),
            Self::Incomplete {
                target,
                observed: recorded,
                error,
            } => json!({
                "status": "incomplete", "target": target,
                "observed": recorded,
                "error": error, "renamed": false
            }),
        }
    }
}

pub(crate) fn rename_prepared_anchor(
    value: &super::binding_slot::BindingValue,
    cwd: &str,
    target: &str,
    is_current_master: bool,
) -> Result<PhysicalAnchorOutcome, String> {
    use crate::client::adapters::tmux;
    if target == "Jenny" && !is_current_master {
        super::tm_layout::require_master_position(value)?;
    }
    let Some(caller) = value.binding.tmux_endpoint.as_ref() else {
        return Ok(PhysicalAnchorOutcome::NotRequested);
    };
    if tmux::iterm2_panes_for_endpoint(caller)?.is_empty() {
        return Ok(PhysicalAnchorOutcome::NotRequested);
    }
    let target_name = tmux_role_label(value, caller, cwd, target)?;
    let previous = match tmux::require_live_session_name(caller) {
        Ok(previous) => previous,
        Err(error) => {
            return Ok(PhysicalAnchorOutcome::Incomplete {
                target: target_name,
                observed: None,
                error,
            });
        }
    };
    if previous == target_name {
        return Ok(PhysicalAnchorOutcome::Consistent { target: target_name });
    }
    if let Err(error) = tmux::rename_session(caller, &target_name) {
        return Ok(PhysicalAnchorOutcome::Incomplete {
            target: target_name,
            observed: tmux::session_name_if_live(caller).ok().flatten(),
            error,
        });
    }
    Ok(PhysicalAnchorOutcome::Applied { target: target_name })
}

fn tmux_role_label(
    value: &super::binding_slot::BindingValue,
    endpoint: &crate::proto::TmuxEndpoint,
    cwd: &str,
    name: &str,
) -> Result<String, String> {
    Ok(super::tm_layout::physical_name(value, cwd, name)?.unwrap_or_else(|| {
        format!("{}_{name}", endpoint.tmux_session_id.trim_start_matches('$'))
    }))
}

pub(crate) fn physical_anchor_projection(
    value: &super::binding_slot::BindingValue,
    name: &str,
) -> serde_json::Value {
    let Some(endpoint) = value.binding.tmux_endpoint.as_ref() else {
        return json!({"status":"not_requested","target":null,"observed":null,"error":null});
    };
    let target = match tmux_role_label(value, endpoint, &value.invocation_cwd, name) {
        Ok(target) => target,
        Err(error) => return json!({"status":"incomplete","target":null,"observed":null,"error":error}),
    };
    match crate::client::adapters::tmux::require_live_session_name(endpoint) {
        Ok(observed) if observed == target => json!({"status":"complete","target":target,"observed":observed,"error":null}),
        Ok(observed) => json!({"status":"incomplete","target":target,"observed":observed,
            "error":format!("TMUX_SESSION_NAME_MISMATCH: expected {target}, observed {observed}")}),
        Err(error) => json!({"status":"incomplete","target":target,"observed":null,"error":error}),
    }
}

pub(crate) fn verify_tmux_replacement(
    previous: Option<&super::binding_slot::BindingValue>,
    candidate: &super::binding_slot::BindingValue,
    explicitly_confirmed: bool,
) -> Result<(), String> {
    use crate::client::adapters::tmux;
    let same_agent = previous
        .is_some_and(|previous| previous.binding.agent_id == candidate.binding.agent_id);
    let Some(candidate) = candidate.transport.as_ref()
        .filter(|transport| transport.kind == crate::proto::TransportKind::Tmux)
        .and_then(|transport| transport.tmux_endpoint.as_ref()) else {
        return Ok(());
    };
    let previous = previous.and_then(|value| value.binding.tmux_endpoint.as_ref());
    if previous.is_some_and(|previous| tmux::same_session(previous, candidate)) {
        return Ok(());
    }
    if tmux::iterm2_panes_for_endpoint(candidate)?.is_empty() {
        return Err("TMUX_ITERM2_PANE_REQUIRED: the new tmux session must already be attached in iTerm2".into());
    }
    if let Some(previous) = previous {
        if !same_agent && !explicitly_confirmed && !tmux::iterm2_panes_for_endpoint(previous)?.is_empty() {
            return Err("TMUX_NAME_ONLINE_CONFLICT: the current tmux session is attached in iTerm2; explicit takeover is required".into());
        }
    }
    Ok(())
}
