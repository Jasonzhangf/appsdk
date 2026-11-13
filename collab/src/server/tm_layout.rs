//! Optional, read-only connection to tm-iterm's layout manifest.
//! Layout never selects an identity and Collab never writes this file.
use super::binding_slot::BindingValue;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug)]
enum LayoutError {
    Ambiguous,
    Other(String),
}

impl LayoutError {
    fn message(self) -> String {
        match self {
            Self::Ambiguous => "TM_LAYOUT_AMBIGUOUS: layout binding is not unique".into(),
            Self::Other(message) => message,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LayoutBinding {
    pub(crate) binding_id: String,
    pub(crate) tab_id: String,
    pub(crate) pane_order: serde_json::Value,
    #[serde(default)]
    cwd: String,
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
    observe_typed(value).map_err(LayoutError::message)
}

fn observe_typed(value: &BindingValue) -> Result<Option<LayoutBinding>, LayoutError> {
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
            return Err(LayoutError::Other(format!(
                "TM_LAYOUT_READ_FAILED: {}: {error}",
                path.display()
            )))
        }
    };
    let entries: Vec<LayoutBinding> = serde_json::from_slice(&bytes).map_err(|error| {
        LayoutError::Other(format!(
            "TM_LAYOUT_DECODE_FAILED: {}: {error}",
            path.display()
        ))
    })?;
    let project_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.tab_id != "overview" && entry.belongs_to_project(value))
        .collect();
    let saved = if let Some(id) = value.layout_binding_id.as_deref() {
        let mut exact = entries.iter().filter(|entry| entry.binding_id == id);
        match exact.next() {
            Some(saved) => {
                if exact.next().is_some() {
                    return Err(LayoutError::Ambiguous);
                }
                if saved.tab_id == "overview" {
                    None
                } else if saved.belongs_to_project(value) {
                    Some(saved)
                } else {
                    return Err(LayoutError::Other(
                        "TM_LAYOUT_PROJECT_MISMATCH: layout binding belongs to another project"
                            .into(),
                    ));
                }
            }
            None => None,
        }
    } else {
        None
    };
    let mut matches = project_entries.into_iter().filter(|entry| {
        saved.is_some_and(|saved| {
            saved.binding_id == entry.binding_id
                || (!saved.tmux_session.is_empty() && saved.tmux_session == entry.tmux_session)
        }) || value
            .physical_name
            .as_deref()
            .is_some_and(|name| name == entry.tmux_session)
    });
    let Some(entry) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(LayoutError::Ambiguous);
    }
    if entry.binding_id.trim().is_empty() || entry.tab_id.trim().is_empty() {
        return Err(LayoutError::Other(
            "TM_LAYOUT_INVALID: binding_id and tab_id are required".into(),
        ));
    }
    entry.slot().map_err(LayoutError::Other)?;
    Ok(Some(entry.clone()))
}

impl LayoutBinding {
    fn belongs_to_project(&self, value: &BindingValue) -> bool {
        if self.cwd.trim().is_empty() {
            return false;
        }
        let Ok(cwd) = std::fs::canonicalize(&self.cwd) else {
            return false;
        };
        [
            value.binding.project_scope.as_str(),
            value.invocation_cwd.as_str(),
        ]
        .into_iter()
        .any(|root| std::fs::canonicalize(root).is_ok_and(|root| root == cwd))
    }

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
    _cwd: &str,
    name: &str,
) -> Result<Option<String>, String> {
    match observe_typed(value) {
        Err(LayoutError::Ambiguous) => Ok(None),
        Err(error) => Err(error.message()),
        Ok(Some(layout)) => layout.physical_name(name).map(Some),
        // No unique manifest position means no physical target.
        // Ordinary-pool and peer-number labels do not invent a position.
        Ok(None) => Ok(None),
    }
}

pub(crate) fn require_master_position(value: &BindingValue) -> Result<(), String> {
    if let Some(layout) = observe(value)? {
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
