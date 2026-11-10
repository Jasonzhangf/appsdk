use serde_json::Value;

pub(crate) const ISOLATION_WORKTREE: &str = "isolated_worktree";

pub(crate) fn isolation_mode_value_allowed(value: &str) -> bool {
    value == ISOLATION_WORKTREE
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
    fn accepts_git_worktree() {
        assert!(isolation_mode_value_allowed(ISOLATION_WORKTREE));
        assert!(isolation_mode_allowed(&serde_json::json!({
            "isolation_mode": ISOLATION_WORKTREE
        })));
    }

    #[test]
    fn rejects_unknown_or_missing_isolation_mode() {
        assert!(!isolation_mode_value_allowed("layerfs_workspace"));
        assert!(!isolation_mode_value_allowed("other_workspace"));
        assert!(!isolation_mode_allowed(&serde_json::json!({
            "isolation_mode": "layerfs_workspace"
        })));
        assert!(!isolation_mode_allowed(&serde_json::json!({
            "isolation_mode": "other_workspace"
        })));
        assert!(!isolation_mode_allowed(&serde_json::json!({})));
        assert!(!isolation_mode_allowed(&serde_json::json!({
            "isolation_mode": 1
        })));
    }
}
