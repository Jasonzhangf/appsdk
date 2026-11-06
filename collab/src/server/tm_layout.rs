//! Optional, read-only connection to tm-iterm's layout manifest.
//! Layout never selects an identity and Collab never writes this file.
use super::binding_slot::BindingValue;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LayoutBinding {
    pub(crate) binding_id: String,
    pub(crate) tab_id: String,
    pub(crate) pane_order: serde_json::Value,
    #[serde(default)]
    pub(crate) tmux_session: String,
}

fn manifest_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("TMUX_MANIFEST_FILE") {
        return Some(path.into());
    }
    if let Some(root) = std::env::var_os("TM_ITERM_STATE_DIR") {
        return Some(PathBuf::from(root).join("manifest.json"));
    }
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(root.join("tm-iterm/manifest.json"))
}

pub(crate) fn observe(value: &BindingValue) -> Result<Option<LayoutBinding>, String> {
    if value.binding.tmux_endpoint.is_none() {
        return Ok(None);
    }
    let Some(path) = manifest_path() else {
        return Ok(None);
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "TM_LAYOUT_READ_FAILED: {}: {error}",
                path.display()
            ))
        }
    };
    let entries: Vec<LayoutBinding> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("TM_LAYOUT_DECODE_FAILED: {}: {error}", path.display()))?;
    let mut matches =
        entries
            .into_iter()
            .filter(|entry| match value.layout_binding_id.as_deref() {
                Some(id) => entry.binding_id == id,
                None => value
                    .physical_name
                    .as_deref()
                    .is_some_and(|name| name == entry.tmux_session),
            });
    let Some(entry) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err("TM_LAYOUT_AMBIGUOUS: layout binding is not unique".into());
    }
    if entry.binding_id.trim().is_empty() || entry.tab_id.trim().is_empty() {
        return Err("TM_LAYOUT_INVALID: binding_id and tab_id are required".into());
    }
    entry.slot()?;
    Ok(Some(entry))
}

impl LayoutBinding {
    fn slot(&self) -> Result<u64, String> {
        self.pane_order
            .as_u64()
            .or_else(|| self.pane_order.as_str().and_then(|text| text.parse().ok()))
            .filter(|order| *order > 0)
            .ok_or_else(|| "TM_LAYOUT_INVALID: pane_order must be positive".into())
    }

    fn physical_name(&self, name: &str) -> Result<String, String> {
        Ok(format!("{}_{}_{}", self.tab_id, self.slot()?, name))
    }
}

pub(crate) fn physical_name(
    value: &BindingValue,
    cwd: &str,
    name: &str,
) -> Result<Option<String>, String> {
    match observe(value) {
        Ok(Some(layout)) => layout.physical_name(name).map(Some),
        Ok(None) if value.layout_binding_id.is_none() => {
            super::name_resolution::physical_tmux_name(cwd, name).map(Some)
        }
        // A stale/malformed optional layout must not veto a valid identity or
        // invent a new position. Its explicit status is projected separately.
        Ok(None) | Err(_) => Ok(None),
    }
}

pub(crate) fn require_master_position(value: &BindingValue) -> Result<(), String> {
    if let Ok(Some(layout)) = observe(value) {
        if layout.slot()? != 1 {
            return Err(
                "MASTER_SLOT_REQUIRED: only fixed layout position 1 may promote to Jenny".into(),
            );
        }
    }
    Ok(())
}

pub(crate) fn projection(value: &BindingValue) -> serde_json::Value {
    match observe(value) {
        Ok(Some(layout)) => serde_json::json!({"status":"linked", "binding_id":layout.binding_id,
            "tab_id":layout.tab_id, "pane_order":layout.pane_order}),
        Ok(None) => {
            serde_json::json!({"status":if value.layout_binding_id.is_some() {"unavailable"} else {"unlinked"}})
        }
        Err(error) => serde_json::json!({"status":"error","error":error}),
    }
}
