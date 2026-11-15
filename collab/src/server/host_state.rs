//! Host command receipts belong only to the actual typed control owner.
//! Startup does not load or migrate project State.

use std::path::PathBuf;

pub(crate) fn host_state_path(paths: &crate::scope::HostPaths) -> PathBuf {
    // Reserved old path is retained only as a cleanup exclusion, never read.
    paths.state_root().join("host-state.json")
}

#[cfg(test)]
mod tests {
    use super::super::{current_state, host_control::HostControl, HostRouteRecord, State};
    use super::*;
    use crate::scope::HostPaths;

    fn test_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "collab-host-state-{name}-{}-{}",
            std::process::id(),
            super::super::state::now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn approved_host_ablation_startup_does_not_publish_project_control() {
        let root = test_root("approved-no-project-control");
        let paths = HostPaths::for_state_root(root.join("host")).unwrap();
        paths.ensure_root().unwrap();
        HostControl::read_current(&paths).unwrap();
        let published = paths.state_root().join("host-control.json").exists();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(
            !published,
            "ordinary host startup published extra host-control.json"
        );
    }

    #[test]
    fn approved_host_ablation_routes_do_not_require_resident_provenance() {
        let root = test_root("approved-no-resident-provenance");
        let paths = HostPaths::for_state_root(root.join("host")).unwrap();
        paths.ensure_root().unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(project.join(".agent-collab")).unwrap();
        super::super::binding_store::publish_route_records(
            &paths.state_root().join("routes.json"),
            &[HostRouteRecord {
                app_scope_id: crate::identity::CLI_APP_SERVER_ID.into(),
                project_scope: project.to_string_lossy().into_owned(),
                canonical_root: project.to_string_lossy().into_owned(),
                storage_root: project.to_string_lossy().into_owned(),
                registered_ms: 1,
            }],
        )
        .unwrap();
        let result = HostControl::read_current(&paths).map(|_| ());
        std::fs::remove_dir_all(&root).unwrap();
        assert!(
            result.is_ok(),
            "fixed routes must not require a resident migration source: {result:?}"
        );
    }

    #[test]
    fn approved_host_ablation_project_receipt_is_not_a_host_receipt() {
        let root = test_root("approved-project-ledger");
        let paths = HostPaths::for_state_root(root.join("host")).unwrap();
        paths.ensure_root().unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(project.join(".agent-collab/server")).unwrap();
        let project = project.canonicalize().unwrap();
        let mut state = State::default();
        state
            .global
            .record_command(
                crate::identity::CommandId::new("project-only-command").unwrap(),
                crate::identity::OperationId::new("project-only-operation").unwrap(),
                serde_json::json!({"project_only":"opaque-result"}),
            )
            .unwrap();
        state.sequence = state.global.sequence;
        state.revision = state.global.revision;
        let source = project.join(".agent-collab/server/state.json");
        assert!(current_state::save(&source, &state).unwrap());
        let original = std::fs::read(&source).unwrap();
        std::fs::write(
            paths.state_root().join("service-record.json"),
            serde_json::to_vec(&serde_json::json!({"service_scope_root":project})).unwrap(),
        )
        .unwrap();
        let loaded = HostControl::read_current(&paths).unwrap();
        let imported = loaded
            .state
            .lock()
            .unwrap()
            .commands
            .receipts
            .contains_key("project-only-command");
        let after = std::fs::read(&source).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            after, original,
            "startup rewrote the project's actual receipt owner"
        );
        assert!(
            !imported,
            "startup projected an unrelated project receipt into host commands"
        );
    }
}
