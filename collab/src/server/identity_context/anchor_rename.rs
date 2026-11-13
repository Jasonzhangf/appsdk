pub(crate) struct TmuxAnchorRename {
    caller: crate::proto::TmuxEndpoint,
    caller_previous: String,
    target: String,
}

impl TmuxAnchorRename {
    pub(crate) fn rollback(&self) -> Result<(), String> {
        crate::client::adapters::tmux::rename_session(&self.caller, &self.caller_previous)
    }
}

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
    /// The anchor was renamed; the guard can roll the effect back.
    Applied(TmuxAnchorRename),
    /// A reliable target exists but the actual rename/readback failed.
    Incomplete {
        target: String,
        observed: Option<String>,
        error: String,
    },
}

impl PhysicalAnchorOutcome {
    pub(crate) fn rollback(&self) -> Result<(), String> {
        match self {
            Self::Applied(rename) => rename.rollback(),
            Self::Consistent { .. } | Self::NotRequested | Self::Incomplete { .. } => Ok(()),
        }
    }

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
            Self::Applied(rename) => json!({
                "status": "complete", "target": rename.target, "observed": rename.target,
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
    let Some(target_name) = super::tm_layout::physical_name(value, cwd, target)? else {
        return Ok(PhysicalAnchorOutcome::NotRequested);
    };
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
    Ok(PhysicalAnchorOutcome::Applied(TmuxAnchorRename {
        caller: caller.clone(),
        caller_previous: previous,
        target: target_name,
    }))
}

pub(crate) fn physical_anchor_projection(
    value: &super::binding_slot::BindingValue,
    name: &str,
) -> serde_json::Value {
    let target = match super::tm_layout::physical_name(value, &value.invocation_cwd, name) {
        Ok(Some(target)) => target,
        Ok(None) => return json!({"status":"not_requested","target":null,"observed":value.physical_name,"error":null}),
        Err(error) => return json!({"status":"incomplete","target":null,"observed":null,"error":error}),
    };
    let Some(endpoint) = value.binding.tmux_endpoint.as_ref() else {
        return json!({"status":"not_requested","target":null,"observed":null,"error":null});
    };
    match crate::client::adapters::tmux::require_live_session_name(endpoint) {
        Ok(observed) if observed == target => json!({"status":"complete","target":target,"observed":observed,"error":null}),
        Ok(observed) => json!({"status":"incomplete","target":target,"observed":observed,
            "error":format!("TMUX_SESSION_NAME_MISMATCH: expected {target}, observed {observed}")}),
        Err(error) => json!({"status":"incomplete","target":target,"observed":null,"error":error}),
    }
}
