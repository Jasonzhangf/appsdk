pub(crate) struct TmuxAnchorRename {
    caller: crate::proto::TmuxEndpoint,
    caller_previous: String,
    target: String,
    outgoing: Option<(crate::proto::TmuxEndpoint, String)>,
}

impl TmuxAnchorRename {
    pub(crate) fn rollback(&self) -> Result<(), String> {
        use crate::client::adapters::tmux;
        let Some((outgoing, previous)) = &self.outgoing else {
            return tmux::rename_session(&self.caller, &self.caller_previous);
        };
        let observed = tmux::require_live_session_name(outgoing)?;
        tmux::rename_session(&self.caller, &anchor_swap_name())?;
        if let Err(error) = tmux::rename_session(outgoing, previous) {
            let _ = tmux::rename_session(&self.caller, &self.target);
            return Err(error);
        }
        if let Err(error) = tmux::rename_session(&self.caller, &self.caller_previous) {
            let _ = tmux::rename_session(outgoing, &observed);
            let _ = tmux::rename_session(&self.caller, &self.target);
            return Err(error);
        }
        Ok(())
    }
}

fn anchor_swap_name() -> String {
    format!("collab-anchor-{}-{}", std::process::id(), now_ms())
}

pub(crate) fn rename_prepared_anchor(
    value: &super::binding_slot::BindingValue,
    cwd: &str,
    target: &str,
    is_current_master: bool,
    outgoing: Option<(&super::binding_slot::BindingValue, &str)>,
) -> Result<Option<TmuxAnchorRename>, String> {
    use crate::client::adapters::tmux;
    if target == "Jenny" && !is_current_master {
        super::tm_layout::require_master_position(value)?;
    }
    let mut changes = Vec::new();
    if let Some(endpoint) = &value.binding.tmux_endpoint {
        let desired = super::tm_layout::physical_name(value, cwd, target)?
            .ok_or("TM_LAYOUT_UNAVAILABLE: current fixed position cannot be named")?;
        let previous = tmux::require_live_session_name(endpoint)?;
        if previous != desired { changes.push((endpoint.clone(), previous, desired)); }
    }
    if let Some((outgoing, name)) = outgoing {
        if let Some(endpoint) = &outgoing.binding.tmux_endpoint {
            if let Some(previous) = tmux::session_name_if_live(endpoint)? {
                let desired = super::tm_layout::physical_name(outgoing, &outgoing.invocation_cwd, name)?
                    .ok_or("TM_LAYOUT_UNAVAILABLE: outgoing fixed position cannot be named")?;
                if previous != desired { changes.push((endpoint.clone(), previous, desired)); }
            }
        }
    }
    let mut changes = changes.into_iter();
    let Some((caller, previous, target_name)) = changes.next() else { return Ok(None); };
    if let Some((endpoint, old_name, outgoing_name)) = changes.next() {
        tmux::rename_session(&caller, &anchor_swap_name())?;
        if let Err(error) = tmux::rename_session(&endpoint, &outgoing_name) {
            return Err(match tmux::rename_session(&caller, &previous) {
                Ok(()) => error,
                Err(rollback) => format!("{error}; TMUX_SESSION_RENAME_ROLLBACK_FAILED: {rollback}"),
            });
        }
        if let Err(error) = tmux::rename_session(&caller, &target_name) {
            let rollback = tmux::rename_session(&endpoint, &old_name)
                .and_then(|()| tmux::rename_session(&caller, &previous));
            return Err(match rollback {
                Ok(()) => error,
                Err(rollback) => format!("{error}; TMUX_SESSION_RENAME_ROLLBACK_FAILED: {rollback}"),
            });
        }
        return Ok(Some(TmuxAnchorRename { caller, caller_previous: previous, target: target_name,
            outgoing: Some((endpoint, old_name)) }));
    }
    tmux::rename_session(&caller, &target_name)?;
    Ok(Some(TmuxAnchorRename { caller, caller_previous: previous, target: target_name, outgoing: None }))
}
