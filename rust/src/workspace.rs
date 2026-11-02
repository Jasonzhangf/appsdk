use serde_json::Value;

pub(crate) const ISOLATION_WORKTREE: &str = "isolated_worktree";
pub(crate) const ISOLATION_LAYERFS_WORKSPACE: &str = "layerfs_workspace";

pub(crate) fn isolation_mode_value_allowed(value: &str) -> bool {
    matches!(value, ISOLATION_WORKTREE | ISOLATION_LAYERFS_WORKSPACE)
}

pub(crate) fn isolation_mode_allowed(record: &Value) -> bool {
    record
        .get("isolation_mode")
        .and_then(Value::as_str)
        .is_some_and(isolation_mode_value_allowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_git_worktree_and_layerfs_workspace() {
        assert!(isolation_mode_value_allowed(ISOLATION_WORKTREE));
        assert!(isolation_mode_value_allowed(ISOLATION_LAYERFS_WORKSPACE));
        assert!(isolation_mode_allowed(&serde_json::json!({
            "isolation_mode": ISOLATION_WORKTREE
        })));
        assert!(isolation_mode_allowed(&serde_json::json!({
            "isolation_mode": ISOLATION_LAYERFS_WORKSPACE
        })));
    }

    #[test]
    fn rejects_unknown_or_missing_isolation_mode() {
        assert!(!isolation_mode_value_allowed("other_workspace"));
        assert!(!isolation_mode_allowed(&serde_json::json!({
            "isolation_mode": "other_workspace"
        })));
        assert!(!isolation_mode_allowed(&serde_json::json!({})));
    }
}
