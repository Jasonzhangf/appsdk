//! Foundation segment of `appsdk verify` expressed as a DAGpipe graph.
//!
//! This module owns four read-only capability operators and the per-invocation
//! typed control resource they share. It does not own the `verify_internal`
//! command boundary, tests governance, or review preflight. The thin adapters
//! that keep the existing `fail`-based callers working live with the single
//! returning core of each helper in the owning modules.

use super::*;
use pipeline_runtime::{
    compile, parse_graph_json, Cancellation, EffectReplay, ErrorKind, Identity, Operator,
    OperatorContext, Registry, Runtime, ValueType,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) const VERIFY_FOUNDATION_GRAPH: &str =
    include_str!("../../../contracts/dagpipe/verify-foundation.graph.json");

/// Immutable per-invocation control resource for the verify foundation graph.
///
/// `root` is the single project path used by every operator; `admission`
/// records the requested selector and `reset_epoch` the reset-governance read
/// performed once by the caller. Control fields stay out of the JSON payload,
/// so the graph only ever carries the parsed project value.
#[derive(Debug)]
pub(super) struct VerifyInvocation {
    root: PathBuf,
    // Held for the invocation boundary; this capability slice never branches
    // on mode or reset, so the CLI wiring slice is the first real consumer.
    #[allow(dead_code)]
    admission: bool,
    #[allow(dead_code)]
    reset_epoch: bool,
}

impl VerifyInvocation {
    /// Validates root safety and reads the reset record once, then owns both.
    pub(super) fn new(root: PathBuf, admission: bool) -> Result<Self, String> {
        try_assert_project_root_safe(&root)?;
        let reset_epoch = try_reset_governance_record_mode(&root)?.is_some();
        Ok(Self {
            root,
            admission,
            reset_epoch,
        })
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    pub(super) fn reset_epoch(&self) -> bool {
        self.reset_epoch
    }
}

fn project_envelope(input: &Value) -> Result<&Value, String> {
    input
        .get("project")
        .ok_or_else(|| "VERIFY_FOUNDATION_PROJECT_ENVELOPE_MISSING".to_owned())
}

struct LoadProjectOperator {
    invocation: Arc<VerifyInvocation>,
}

impl Operator for LoadProjectOperator {
    fn name(&self) -> &'static str {
        "appsdk.verify.load_project"
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn input_type(&self) -> ValueType {
        ValueType::Null
    }

    fn output_type(&self) -> ValueType {
        ValueType::Object
    }

    fn replay(&self) -> EffectReplay {
        EffectReplay::NonReplayable
    }

    fn execute(&self, _input: Value, _context: &OperatorContext) -> Result<Value, String> {
        let project = try_read_project(self.invocation.root())?;
        Ok(json!({ "project": project }))
    }
}

struct GovernanceBaselineOperator {
    invocation: Arc<VerifyInvocation>,
}

impl Operator for GovernanceBaselineOperator {
    fn name(&self) -> &'static str {
        "appsdk.verify.governance_baseline"
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn input_type(&self) -> ValueType {
        ValueType::Object
    }

    fn output_type(&self) -> ValueType {
        ValueType::Object
    }

    fn replay(&self) -> EffectReplay {
        EffectReplay::NonReplayable
    }

    fn execute(&self, input: Value, _context: &OperatorContext) -> Result<Value, String> {
        try_assert_governance_maps(self.invocation.root())?;
        Ok(input)
    }
}

struct DeclaredContractsOperator {
    invocation: Arc<VerifyInvocation>,
}

impl Operator for DeclaredContractsOperator {
    fn name(&self) -> &'static str {
        "appsdk.verify.declared_contracts"
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn input_type(&self) -> ValueType {
        ValueType::Object
    }

    fn output_type(&self) -> ValueType {
        ValueType::Object
    }

    fn replay(&self) -> EffectReplay {
        EffectReplay::NonReplayable
    }

    fn execute(&self, input: Value, _context: &OperatorContext) -> Result<Value, String> {
        let project = project_envelope(&input)?;
        try_assert_declared_contracts(self.invocation.root(), project)?;
        Ok(input)
    }
}

struct ProjectContractOperator {
    invocation: Arc<VerifyInvocation>,
}

impl Operator for ProjectContractOperator {
    fn name(&self) -> &'static str {
        "appsdk.verify.project_contract"
    }

    fn version(&self) -> &'static str {
        "1"
    }

    fn input_type(&self) -> ValueType {
        ValueType::Object
    }

    fn output_type(&self) -> ValueType {
        ValueType::Object
    }

    fn replay(&self) -> EffectReplay {
        EffectReplay::NonReplayable
    }

    fn execute(&self, input: Value, _context: &OperatorContext) -> Result<Value, String> {
        let project = project_envelope(&input)?;
        try_assert_project_contract(self.invocation.root(), project)?;
        Ok(input)
    }
}

pub(super) fn register_verify_operators(
    registry: &mut Registry,
    invocation: Arc<VerifyInvocation>,
) -> Result<(), String> {
    registry
        .register(LoadProjectOperator {
            invocation: invocation.clone(),
        })
        .map_err(|error| format!("DAGPIPE_OPERATOR_REGISTER_FAILED:{error}"))?;
    registry
        .register(GovernanceBaselineOperator {
            invocation: invocation.clone(),
        })
        .map_err(|error| format!("DAGPIPE_OPERATOR_REGISTER_FAILED:{error}"))?;
    registry
        .register(DeclaredContractsOperator {
            invocation: invocation.clone(),
        })
        .map_err(|error| format!("DAGPIPE_OPERATOR_REGISTER_FAILED:{error}"))?;
    registry
        .register(ProjectContractOperator { invocation })
        .map_err(|error| format!("DAGPIPE_OPERATOR_REGISTER_FAILED:{error}"))?;
    Ok(())
}

/// Compile-only registration for `appsdk dagpipe validate`. It never executes,
/// so the placeholder invocation never touches the filesystem.
pub(super) fn register_verify_operators_for_validation(
    registry: &mut Registry,
) -> Result<(), String> {
    register_verify_operators(
        registry,
        Arc::new(VerifyInvocation {
            root: PathBuf::from("."),
            admission: false,
            reset_epoch: false,
        }),
    )
}

/// Runs the foundation graph for one invocation and returns the `project_checked`
/// envelope for the not-yet-migrated remainder of `verify_internal`.
pub(super) fn run_verify_foundation(invocation: Arc<VerifyInvocation>) -> Result<Value, String> {
    let graph = parse_graph_json(VERIFY_FOUNDATION_GRAPH)
        .map_err(|error| format!("DAGPIPE_GRAPH_INVALID:{error}"))?;
    let mut registry = Registry::default();
    register_verify_operators(&mut registry, invocation)?;
    let capabilities = BTreeSet::new();
    let compiled = compile(graph, &registry, &capabilities)
        .map_err(|error| format!("DAGPIPE_COMPILE_FAILED:{error}"))?;
    let identity = Identity {
        project_id: "appsdk".to_owned(),
        graph_id: compiled.id().to_owned(),
        graph_version: compiled.version().to_owned(),
        execution_id: "verify-foundation".to_owned(),
        attempt_id: "1".to_owned(),
    };
    let mut inputs = HashMap::new();
    inputs.insert("verify_request".to_owned(), Value::Null);
    let result = Runtime::new(capabilities)
        .run(&compiled, identity, inputs, &Cancellation::default())
        .map_err(|failure| match failure.error.kind {
            // Capability errors keep the owning operator's original diagnostic.
            ErrorKind::Operator => failure.error.message,
            // Graph, input, output, or runtime-structure errors get a distinct code.
            _ => format!("DAGPIPE_EXECUTION_FAILED:{failure}"),
        })?;
    result
        .outputs
        .get("project_checked")
        .map(|arc| arc.payload.clone())
        .ok_or_else(|| "DAGPIPE_OUTPUT_MISSING:project_checked".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestProject {
        root: PathBuf,
    }

    impl TestProject {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "appsdk-verify-foundation-{label}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&root).unwrap();
            ensure_governance_layout(&root);
            write_project_scaffold(&root);
            Self { root }
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.root.join(relative)
        }

        fn write_json(&self, relative: &str, value: &Value) {
            let path = self.path(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
        }

        fn read_json(&self, relative: &str) -> Value {
            serde_json::from_str(&fs::read_to_string(self.path(relative)).unwrap()).unwrap()
        }
    }

    impl Drop for TestProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn invocation(project: &TestProject) -> Arc<VerifyInvocation> {
        Arc::new(VerifyInvocation::new(project.root.clone(), false).unwrap())
    }

    #[test]
    fn foundation_accepts_valid_scaffold_and_preserves_project_value() {
        let project = TestProject::new("valid");
        let output = run_verify_foundation(invocation(&project)).unwrap();
        assert_eq!(output.as_object().unwrap().len(), 1);
        assert_eq!(output["project"], project.read_json(".appsdk/project.json"));
    }

    #[test]
    fn invocation_captures_reset_once_without_payload_state() {
        let project = TestProject::new("reset-once");
        project.write_json(
            ".appsdk/records/reset-governance-record.json",
            &json!({
                "schema_version": 1,
                "mode": "discard_legacy_control_plane",
                "reset_id": "reset-123",
                "transaction_id": "reset-123",
                "branch": "codex/reset-once",
                "created_at": "2020-01-01T00:00:00Z",
                "preserved": ["protected"],
                "removed": ["legacy"]
            }),
        );

        let invocation = Arc::new(VerifyInvocation::new(project.root.clone(), false).unwrap());
        assert!(invocation.reset_epoch);
        assert!(!invocation.admission);
        fs::remove_file(project.path(".appsdk/records/reset-governance-record.json")).unwrap();

        let output = run_verify_foundation(invocation).unwrap();
        assert_eq!(
            output
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["project"]
        );
    }

    #[test]
    fn broken_json_fails_at_load_but_legal_non_object_fails_later() {
        let project = TestProject::new("json");
        fs::write(project.path(".appsdk/project.json"), "{ not json").unwrap();
        let load_error = run_verify_foundation(invocation(&project)).unwrap_err();
        assert_eq!(load_error, "INVALID_PROJECT_CONTRACT");

        let project = TestProject::new("nonobject");
        project.write_json(".appsdk/project.json", &json!([1, 2, 3]));
        let semantic_error = run_verify_foundation(invocation(&project)).unwrap_err();
        assert_ne!(semantic_error, "INVALID_PROJECT_CONTRACT");
        assert!(
            semantic_error.starts_with("INVALID_GOVERNANCE_CONTRACT")
                || semantic_error.starts_with("INVALID_PROJECT_CONTRACT"),
            "{semantic_error}"
        );
    }

    #[test]
    fn governance_baseline_rejects_broken_and_missing_maps() {
        let project = TestProject::new("broken-map");
        fs::write(project.path(".appsdk/maps/resource-map.json"), "{ not json").unwrap();
        assert_eq!(
            run_verify_foundation(invocation(&project)).unwrap_err(),
            "INVALID_GOVERNANCE_MAP:resource-map.json"
        );

        let project = TestProject::new("missing-map");
        fs::remove_file(project.path(".appsdk/maps/module-registry.json")).unwrap();
        assert_eq!(
            run_verify_foundation(invocation(&project)).unwrap_err(),
            "MISSING_GOVERNANCE_MAP:module-registry.json"
        );
    }

    #[test]
    fn maps_error_precedes_declared_contract_error() {
        let project = TestProject::new("map-first");
        fs::write(project.path(".appsdk/maps/resource-map.json"), "{ not json").unwrap();
        let mut project_value = project.read_json(".appsdk/project.json");
        project_value["governance"]["record_contracts"] = json!([]);
        project.write_json(".appsdk/project.json", &project_value);
        assert_eq!(
            run_verify_foundation(invocation(&project)).unwrap_err(),
            "INVALID_GOVERNANCE_MAP:resource-map.json"
        );
    }

    #[test]
    fn declared_contracts_rejects_recursive_schema_weakening() {
        let project = TestProject::new("weakened");
        let record_path = "contracts/records/evidence-record.schema.json";
        let mut schema = project.read_json(record_path);
        schema.as_object_mut().unwrap().remove("allOf");
        project.write_json(record_path, &schema);
        let error = run_verify_foundation(invocation(&project)).unwrap_err();
        assert!(
            error.starts_with("DECLARED_RECORD_CONTRACT_MISMATCH")
                || error.starts_with("INVALID_DECLARED_RECORD_CONTRACT"),
            "{error}"
        );
    }

    #[test]
    fn declared_contracts_rejects_non_canonical_record_set() {
        let project = TestProject::new("record-set");
        let mut project_value = project.read_json(".appsdk/project.json");
        project_value["governance"]["record_contracts"] =
            json!(["contracts/records/record-graph.contract.json"]);
        project.write_json(".appsdk/project.json", &project_value);
        assert_eq!(
            run_verify_foundation(invocation(&project)).unwrap_err(),
            "NON_CANONICAL_RECORD_CONTRACT_SET"
        );
    }

    #[test]
    fn project_contract_rejects_overlapping_module_ownership() {
        let project = TestProject::new("overlap");
        let mut project_value = project.read_json(".appsdk/project.json");
        let module = &project_value["modules"][0];
        project_value["modules"] = json!([module, module]);
        project.write_json(".appsdk/project.json", &project_value);
        let error = run_verify_foundation(invocation(&project)).unwrap_err();
        assert!(
            error == "OVERLAPPING_MODULE_OWNERSHIP"
                || error.starts_with("DUPLICATE")
                || error.starts_with("INVALID_PROJECT_MODULE"),
            "{error}"
        );
    }

    #[test]
    fn failure_then_success_in_the_same_process() {
        let broken = TestProject::new("sequence-fail");
        fs::remove_file(broken.path(".appsdk/maps/function-map.json")).unwrap();
        assert_eq!(
            run_verify_foundation(invocation(&broken)).unwrap_err(),
            "MISSING_GOVERNANCE_MAP:function-map.json"
        );

        let valid = TestProject::new("sequence-ok");
        let output = run_verify_foundation(invocation(&valid)).unwrap();
        assert_eq!(output["project"]["project_id"], json!("change-me"));
    }

    #[test]
    fn two_roots_do_not_share_invocation_state() {
        let broken = TestProject::new("root-a");
        fs::write(
            broken.path(".appsdk/maps/verification-map.json"),
            "{ not json",
        )
        .unwrap();
        let valid = TestProject::new("root-b");

        let first = run_verify_foundation(invocation(&broken));
        let second = run_verify_foundation(invocation(&valid));
        assert_eq!(
            first.unwrap_err(),
            "INVALID_GOVERNANCE_MAP:verification-map.json"
        );
        let output = second.unwrap();
        assert_eq!(output["project"]["project_id"], json!("change-me"));
    }

    #[test]
    fn root_and_project_symlink_errors_surface() {
        let missing = std::env::temp_dir().join(format!(
            "appsdk-verify-foundation-missing-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&missing);
        assert_eq!(
            VerifyInvocation::new(missing, false).unwrap_err(),
            "PROJECT_ROOT_MISSING"
        );

        let project = TestProject::new("symlink");
        let real = project.path(".appsdk/project.json.real");
        fs::rename(project.path(".appsdk/project.json"), &real).unwrap();
        std::os::unix::fs::symlink(&real, project.path(".appsdk/project.json")).unwrap();
        assert_eq!(
            run_verify_foundation(invocation(&project)).unwrap_err(),
            "GOVERNANCE_PATH_SYMLINK:project"
        );

        let project = TestProject::new("appsdk-symlink");
        let real = project.path(".appsdk.real");
        fs::rename(project.path(".appsdk"), &real).unwrap();
        std::os::unix::fs::symlink(&real, project.path(".appsdk")).unwrap();
        assert_eq!(
            VerifyInvocation::new(project.root.clone(), false).unwrap_err(),
            "GOVERNANCE_PATH_SYMLINK:appsdk_control"
        );
    }

    #[test]
    fn downstream_operators_consume_the_predecessor_project_value() {
        let project = TestProject::new("predecessor");
        let invocation = invocation(&project);
        let context = OperatorContext {
            identity: Identity {
                project_id: "appsdk".to_owned(),
                graph_id: "appsdk-verify-foundation".to_owned(),
                graph_version: "0.1.0".to_owned(),
                execution_id: "unit".to_owned(),
                attempt_id: "1".to_owned(),
            },
            node_id: "test".to_owned(),
        };
        let loaded = LoadProjectOperator {
            invocation: invocation.clone(),
        }
        .execute(Value::Null, &context)
        .unwrap();
        assert_eq!(loaded["project"]["project_id"], json!("change-me"));

        let mut tampered = loaded;
        tampered["project"] = json!({});
        let declared = DeclaredContractsOperator {
            invocation: invocation.clone(),
        }
        .execute(tampered.clone(), &context)
        .unwrap_err();
        assert_eq!(declared, "INVALID_GOVERNANCE_CONTRACT");
        let project_contract = ProjectContractOperator { invocation }
            .execute(tampered, &context)
            .unwrap_err();
        assert_eq!(project_contract, "INVALID_PROJECT_CONTRACT");
    }

    /// Direct calls to the four operator cores against one isolated copy of a
    /// scaffold are the programmatic baseline; `run_verify_foundation` on a
    /// second identical copy is the graph candidate. Both use the same
    /// no-digest policy, so public result and first error must match exactly.
    fn run_programmatic_baseline(root: &Path) -> Result<Value, String> {
        let project = try_read_project(root)?;
        try_assert_governance_maps(root)?;
        try_assert_declared_contracts(root, &project)?;
        try_assert_project_contract(root, &project)?;
        Ok(json!({ "project": project }))
    }

    #[test]
    fn programmatic_baseline_matches_graph_candidate_on_same_input() {
        let graph_valid = TestProject::new("compare-valid-graph");
        let baseline_valid = TestProject::new("compare-valid-baseline");
        let graph = run_verify_foundation(invocation(&graph_valid)).unwrap();
        let baseline = run_programmatic_baseline(&baseline_valid.root).unwrap();
        assert_eq!(graph, baseline);

        let graph_broken = TestProject::new("compare-broken-graph");
        let baseline_broken = TestProject::new("compare-broken-baseline");
        for project in [&graph_broken, &baseline_broken] {
            fs::write(project.path(".appsdk/maps/resource-map.json"), "{ not json").unwrap();
        }
        let graph_error = run_verify_foundation(invocation(&graph_broken)).unwrap_err();
        let baseline_error = run_programmatic_baseline(&baseline_broken.root).unwrap_err();
        assert_eq!(graph_error, baseline_error);
        assert_eq!(graph_error, "INVALID_GOVERNANCE_MAP:resource-map.json");
    }

    /// One endpoint calls the four owner `try_*` helpers directly; the other
    /// runs the migrated DAG. The two paths never share the graph runtime, so
    /// agreement is an independent before/after comparison of the foundation
    /// segment. The digest-ablated old migration-record check is intentionally
    /// absent from the baseline: it was summary logic removed by this change.
    fn assert_endpoints_agree(
        label: &str,
        mutate: impl Fn(&TestProject),
    ) -> (Result<Value, String>, Result<Value, String>) {
        let graph = TestProject::new(&format!("{label}-graph"));
        let baseline = TestProject::new(&format!("{label}-baseline"));
        mutate(&graph);
        mutate(&baseline);
        let graph_result = run_verify_foundation(invocation(&graph));
        let baseline_result = run_programmatic_baseline(&baseline.root);
        assert_eq!(graph_result, baseline_result, "{label}");
        (graph_result, baseline_result)
    }

    #[test]
    fn same_input_baseline_and_graph_agree_across_foundation_outcomes() {
        let (graph, baseline) = assert_endpoints_agree("matrix-valid", |_| {});
        assert_eq!(graph.unwrap(), baseline.unwrap());

        let (graph, _) = assert_endpoints_agree("matrix-broken-json", |project| {
            fs::write(project.path(".appsdk/project.json"), "{ not json").unwrap();
        });
        assert_eq!(graph.unwrap_err(), "INVALID_PROJECT_CONTRACT");

        let (graph, _) = assert_endpoints_agree("matrix-nonobject", |project| {
            project.write_json(".appsdk/project.json", &json!([1, 2, 3]));
        });
        let error = graph.unwrap_err();
        assert_ne!(error, "INVALID_PROJECT_CONTRACT");
        assert!(
            error.starts_with("INVALID_GOVERNANCE_CONTRACT")
                || error.starts_with("INVALID_PROJECT_CONTRACT"),
            "{error}"
        );

        let (graph, _) = assert_endpoints_agree("matrix-broken-map", |project| {
            fs::write(project.path(".appsdk/maps/resource-map.json"), "{ not json").unwrap();
        });
        assert_eq!(
            graph.unwrap_err(),
            "INVALID_GOVERNANCE_MAP:resource-map.json"
        );

        let (graph, _) = assert_endpoints_agree("matrix-missing-map", |project| {
            fs::remove_file(project.path(".appsdk/maps/module-registry.json")).unwrap();
        });
        assert_eq!(
            graph.unwrap_err(),
            "MISSING_GOVERNANCE_MAP:module-registry.json"
        );

        let (graph, _) = assert_endpoints_agree("matrix-map-first", |project| {
            fs::write(project.path(".appsdk/maps/resource-map.json"), "{ not json").unwrap();
            let mut project_value = project.read_json(".appsdk/project.json");
            project_value["governance"]["record_contracts"] = json!([]);
            project.write_json(".appsdk/project.json", &project_value);
        });
        assert_eq!(
            graph.unwrap_err(),
            "INVALID_GOVERNANCE_MAP:resource-map.json"
        );

        let (graph, _) = assert_endpoints_agree("matrix-weakened-schema", |project| {
            let record_path = "contracts/records/evidence-record.schema.json";
            let mut schema = project.read_json(record_path);
            schema.as_object_mut().unwrap().remove("allOf");
            project.write_json(record_path, &schema);
        });
        let error = graph.unwrap_err();
        assert!(
            error.starts_with("DECLARED_RECORD_CONTRACT_MISMATCH")
                || error.starts_with("INVALID_DECLARED_RECORD_CONTRACT"),
            "{error}"
        );

        let (graph, _) = assert_endpoints_agree("matrix-noncanonical-record", |project| {
            let mut project_value = project.read_json(".appsdk/project.json");
            project_value["governance"]["record_contracts"] =
                json!(["contracts/records/record-graph.contract.json"]);
            project.write_json(".appsdk/project.json", &project_value);
        });
        assert_eq!(graph.unwrap_err(), "NON_CANONICAL_RECORD_CONTRACT_SET");

        let (graph, _) = assert_endpoints_agree("matrix-overlap", |project| {
            let mut project_value = project.read_json(".appsdk/project.json");
            let module = &project_value["modules"][0];
            project_value["modules"] = json!([module, module]);
            project.write_json(".appsdk/project.json", &project_value);
        });
        let error = graph.unwrap_err();
        assert!(
            error == "OVERLAPPING_MODULE_OWNERSHIP"
                || error.starts_with("DUPLICATE")
                || error.starts_with("INVALID_PROJECT_MODULE"),
            "{error}"
        );

        let (graph, baseline) = assert_endpoints_agree("matrix-reset", |project| {
            project.write_json(
                ".appsdk/records/reset-governance-record.json",
                &json!({
                    "schema_version": 1,
                    "mode": "discard_legacy_control_plane",
                    "reset_id": "reset-matrix",
                    "transaction_id": "reset-matrix",
                    "branch": "codex/matrix",
                    "created_at": "2020-01-01T00:00:00Z",
                    "preserved": ["protected"],
                    "removed": ["legacy"]
                }),
            );
        });
        assert_eq!(graph.unwrap(), baseline.unwrap());
    }

    #[test]
    fn foundation_outcome_is_admission_invariant() {
        let valid = TestProject::new("admission-valid");
        let default_result = run_verify_foundation(invocation(&valid));
        let admission_result = run_verify_foundation(Arc::new(
            VerifyInvocation::new(valid.root.clone(), true).unwrap(),
        ));
        assert_eq!(default_result, admission_result);

        let broken = TestProject::new("admission-broken");
        fs::write(broken.path(".appsdk/maps/resource-map.json"), "{ not json").unwrap();
        let default_error = run_verify_foundation(invocation(&broken)).unwrap_err();
        let admission_error = run_verify_foundation(Arc::new(
            VerifyInvocation::new(broken.root.clone(), true).unwrap(),
        ))
        .unwrap_err();
        assert_eq!(default_error, admission_error);
        assert_eq!(default_error, "INVALID_GOVERNANCE_MAP:resource-map.json");
    }

    #[test]
    fn graph_run_leaves_project_tree_unchanged() {
        fn snapshot(
            dir: &Path,
            base: &Path,
            entries: &mut Vec<(String, u64, std::time::SystemTime)>,
        ) {
            for entry in fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).unwrap();
                let relative = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                if metadata.is_dir() {
                    snapshot(&path, base, entries);
                } else {
                    entries.push((relative, metadata.len(), metadata.modified().unwrap()));
                }
            }
        }

        let project = TestProject::new("read-only");
        let mut before = Vec::new();
        snapshot(&project.root, &project.root, &mut before);
        before.sort();
        run_verify_foundation(invocation(&project)).unwrap();
        let mut after = Vec::new();
        snapshot(&project.root, &project.root, &mut after);
        after.sort();
        assert_eq!(before, after, "graph run changed the project tree");
    }
}
