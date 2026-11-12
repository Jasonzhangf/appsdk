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
/// `root` is the single project path used by every operator; `reset_epoch`
/// records the reset-governance read performed once by the caller. Control
/// fields stay out of the JSON payload, so the graph only ever carries the
/// parsed project value.
#[derive(Debug)]
pub(super) struct VerifyInvocation {
    root: PathBuf,
    #[allow(dead_code)]
    reset_epoch: bool,
}

impl VerifyInvocation {
    /// Validates root safety and reads the reset record once, then owns both.
    pub(super) fn new(root: PathBuf) -> Result<Self, String> {
        try_assert_project_root_safe(&root)?;
        let reset_epoch = try_reset_governance_record_mode(&root)?.is_some();
        Ok(Self { root, reset_epoch })
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
        Arc::new(VerifyInvocation::new(project.root.clone()).unwrap())
    }

    #[test]
    fn foundation_accepts_valid_scaffold_and_preserves_project_value() {
        let project = TestProject::new("valid");
        let output = run_verify_foundation(invocation(&project)).unwrap();
        assert_eq!(output.as_object().unwrap().len(), 1);
        assert_eq!(output["project"], project.read_json(".appsdk/project.json"));
    }

    #[test]
    fn returning_string_and_identifier_contracts_have_exact_boundaries() {
        assert_eq!(
            try_required_str(&json!({"value": ""}), "/value", "MISSING"),
            Ok("")
        );
        assert_eq!(
            try_required_str(&json!({}), "/value", "MISSING"),
            Err("MISSING".to_owned())
        );
        assert_eq!(
            try_required_str(&json!({"value": 1}), "/value", "MISSING"),
            Err("MISSING".to_owned())
        );
        assert_eq!(
            try_record_str(&json!({"value": "present"}), "/value", "sample"),
            Ok("present")
        );
        for record in [json!({}), json!({"value": ""}), json!({"value": 1})] {
            assert_eq!(
                try_record_str(&record, "/value", "sample"),
                Err("INVALID_RECORD:sample:/value".to_owned())
            );
        }
        assert_eq!(
            try_record_array(&json!({"items": [1]}), "/items", "sample")
                .unwrap()
                .len(),
            1
        );
        for record in [json!({}), json!({"items": []}), json!({"items": "x"})] {
            assert_eq!(
                try_record_array(&record, "/items", "sample"),
                Err("INVALID_RECORD:sample:/items".to_owned())
            );
        }
        for value in ["a", "a-1", "z9"] {
            assert_eq!(try_assert_identifier(value, "BAD_ID"), Ok(()));
        }
        for value in ["", "A", "1a", "a_b", "a.b"] {
            assert_eq!(
                try_assert_identifier(value, "BAD_ID"),
                Err("BAD_ID".to_owned())
            );
        }
    }

    #[test]
    fn safe_roots_and_owned_paths_reject_escape_and_symlinks() {
        let project = TestProject::new("safe-paths");
        let value = project.read_json(".appsdk/project.json");
        assert_eq!(
            try_contract_root(&project.root, &value, "/governance/generated_root").unwrap(),
            project.root.join("generated")
        );
        for root in [".", "../outside", "/tmp/outside"] {
            let mut root_value = value.clone();
            root_value["governance"]["generated_root"] = json!(root);
            assert_eq!(
                try_contract_root(&project.root, &root_value, "/governance/generated_root"),
                Err("INVALID_GOVERNANCE_ROOT:/governance/generated_root".to_owned())
            );
        }
        for relative in ["playground", "active/lib/**", "."] {
            assert!(try_safe_owned_path(&project.root, relative, "owned").is_ok());
        }
        for relative in ["", "../outside", "a/../outside", "/tmp/outside"] {
            assert_eq!(
                try_safe_owned_path(&project.root, relative, "owned"),
                Err("INVALID_OWNED_PATH:owned".to_owned())
            );
        }
        assert_eq!(
            try_assert_no_symlink_components(&project.root, Path::new("/tmp/outside"), "owned"),
            Err("GOVERNANCE_PATH_ESCAPE:owned".to_owned())
        );

        #[cfg(unix)]
        {
            let outside = project.root.with_extension("outside");
            fs::create_dir_all(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, project.path("linked")).unwrap();
            assert_eq!(
                try_safe_owned_path(&project.root, "linked/child", "owned"),
                Err("GOVERNANCE_PATH_SYMLINK:owned".to_owned())
            );
            fs::remove_dir_all(outside).unwrap();
        }
    }

    #[test]
    fn development_scenario_set_contract_reports_exact_failures() {
        let cases: [(&str, Value, &'static str); 5] = [
            ("canonical-empty", json!([]), "OK"),
            (
                "valid-pair",
                json!(["multi_worker_collaboration", "multi_worktree_merge_queue"]),
                "OK",
            ),
            (
                "duplicate",
                json!(["multi_worker_collaboration", "multi_worker_collaboration"]),
                "DUPLICATE_DEVELOPMENT_SCENARIO",
            ),
            ("unknown", json!(["other"]), "UNKNOWN_DEVELOPMENT_SCENARIO"),
            (
                "merge-without-collaboration",
                json!(["multi_worktree_merge_queue"]),
                "MERGE_QUEUE_COLLABORATION_REQUIRED",
            ),
        ];
        for (label, enabled, expected) in cases {
            let project = TestProject::new(label);
            let mut value = project.read_json(".appsdk/project.json");
            value["development_scenarios"]["enabled"] = enabled;
            if expected == "OK" {
                assert!(try_assert_development_scenarios(&project.root, &value).is_ok());
            } else {
                match try_assert_development_scenarios(&project.root, &value) {
                    Err(error) => assert_eq!(error, expected),
                    Ok(_) => panic!("scenario case unexpectedly succeeded: {label}"),
                }
            }
        }
        let project = TestProject::new("manifest-mismatch");
        let mut value = project.read_json(".appsdk/project.json");
        fs::write(
            project.path(".appsdk/contracts/development-scenarios.manifest.json"),
            "{}",
        )
        .unwrap();
        assert_eq!(
            match try_assert_development_scenarios(&project.root, &value) {
                Err(error) => error,
                Ok(_) => panic!("scenario manifest unexpectedly succeeded"),
            },
            "DEVELOPMENT_SCENARIO_MANIFEST_MISMATCH"
        );
        value["development_scenarios"]["manifest"] = json!("contracts/other.json");
        assert_eq!(
            match try_assert_development_scenarios(&project.root, &value) {
                Err(error) => error,
                Ok(_) => panic!("scenario path unexpectedly succeeded"),
            },
            "NON_CANONICAL_DEVELOPMENT_SCENARIO_MANIFEST"
        );
    }

    #[test]
    fn maps_require_nonempty_fields_and_complete_mainline_edges() {
        let project = TestProject::new("maps-contract");
        assert_eq!(try_assert_governance_maps(&project.root), Ok(()));
        for (name, value, expected) in [
            (
                "resource-map.json",
                json!({"schema_version": 2, "resources": ["x"]}),
                "INVALID_GOVERNANCE_MAP:resource-map.json",
            ),
            (
                "function-map.json",
                json!({"schema_version": 1, "functions": []}),
                "INVALID_GOVERNANCE_MAP:function-map.json",
            ),
            (
                "mainline-call-map.json",
                json!({"schema_version": 1, "edges": [{
                    "chain_id":"c", "owner":"o", "caller":"a", "callee":"b",
                    "path":"p", "input_resource_id":"i", "error_resource_id":"e"
                }]}),
                "INVALID_RECORD:mainline-call-map.json:/output_resource_id",
            ),
            (
                "mainline-call-map.json",
                json!({"schema_version": 1, "edges": [{
                    "chain_id":"c", "owner":"o", "caller":"a", "callee":"b",
                    "path":"p", "input_resource_id":"i", "output_resource_id":"o",
                    "error_resource_id":"e"
                }]}),
                "OK",
            ),
        ] {
            let project = TestProject::new(name);
            project.write_json(&format!(".appsdk/maps/{name}"), &value);
            let result = try_assert_governance_maps(&project.root);
            if expected == "OK" {
                assert_eq!(result, Ok(()));
            } else {
                assert_eq!(result.unwrap_err(), expected);
            }
        }
    }

    #[test]
    fn recursive_record_schema_accepts_strengthening_and_rejects_weakening() {
        let canonical = json!({
            "type": "object",
            "required": ["kind"],
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["a", "b"],
                    "minLength": 1,
                    "if": {"properties": {"kind": {"const": "a"}}},
                    "then": {"required": ["reason"]},
                    "not": {"required": ["forbidden"]},
                    "allOf": [{"properties": {"kind": {"type": "string"}}}]
                },
                "items": {"type": "array", "items": {"minimum": 1}}
            }
        });
        let strengthened = json!({
            "type": "object",
            "required": ["kind", "extra"],
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["a"],
                    "minLength": 2,
                    "if": {"properties": {"kind": {"const": "a"}}},
                    "then": {"required": ["reason", "detail"]},
                    "not": {"required": ["forbidden", "other"]},
                    "allOf": [{"properties": {"kind": {"type": "string", "minLength": 2}}}]
                },
                "items": {"type": "array", "items": {"minimum": 2}}
            }
        });
        assert_eq!(
            try_assert_schema_property_compatible(&canonical, &strengthened, "sample"),
            Ok(())
        );
        let weakened: [(&str, fn(&mut Value)); 8] = [
            ("required", |schema| {
                schema["required"] = json!([]);
            }),
            ("bounds", |schema| {
                schema["properties"]["kind"]["minLength"] = json!(0);
            }),
            ("enum", |schema| {
                schema["properties"]["kind"]["enum"] = json!(["a", "b", "c"]);
            }),
            ("items", |schema| {
                schema["properties"]["items"]["items"]["minimum"] = json!(0);
            }),
            ("if", |schema| {
                schema["properties"]["kind"]
                    .as_object_mut()
                    .unwrap()
                    .remove("if");
            }),
            ("then", |schema| {
                schema["properties"]["kind"]["then"]["required"] = json!([]);
            }),
            ("not", |schema| {
                schema["properties"]["kind"]
                    .as_object_mut()
                    .unwrap()
                    .remove("not");
            }),
            ("allOf", |schema| {
                schema["properties"]["kind"]
                    .as_object_mut()
                    .unwrap()
                    .remove("allOf");
            }),
        ];
        for (label, weaken) in weakened {
            let mut schema = strengthened.clone();
            weaken(&mut schema);
            assert!(
                try_assert_schema_property_compatible(&canonical, &schema, "sample").is_err(),
                "weakened {label} constraint was accepted: {schema}"
            );
        }

        let promotion =
            try_canonical_record_contract("contracts/records/promotion-record.schema.json")
                .unwrap();
        let mut compatible = promotion.clone();
        compatible["required"]
            .as_array_mut()
            .unwrap()
            .retain(|name| name != "bug_closure_verified");
        compatible["properties"]
            .as_object_mut()
            .unwrap()
            .remove("bug_closure_verified");
        assert_eq!(
            try_assert_record_schema_minimum(
                "contracts/records/promotion-record.schema.json",
                &compatible
            ),
            Ok(())
        );
    }

    #[test]
    fn declared_zone_transitions_accept_aliases_and_reject_contract_changes() {
        for alias in [
            "contracts/transitions/zone-transition.manifest.json",
            "contracts/transitions/zone-transition-manifest.json",
        ] {
            let project = TestProject::new("transition-alias");
            let mut value = project.read_json(".appsdk/project.json");
            value["governance"]["zone_transition_contract"] = json!(alias);
            assert_eq!(try_assert_declared_contracts(&project.root, &value), Ok(()));
        }
        for (label, mutate, expected) in [
            ("transition-missing", 0, "INVALID_DECLARED_ZONE_CONTRACT"),
            ("transition-duplicate", 1, "INVALID_DECLARED_ZONE_CONTRACT"),
            ("transition-changed", 2, "INVALID_DECLARED_ZONE_CONTRACT"),
        ] {
            let project = TestProject::new(label);
            let path = project.path("contracts/transitions/zone-transition.manifest.json");
            let mut zone: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            match mutate {
                0 => zone["transitions"] = json!("bad"),
                1 => {
                    let first = zone["transitions"][0].clone();
                    zone["transitions"].as_array_mut().unwrap().push(first);
                }
                _ => zone["transitions"][0]["owner"] = json!("other"),
            }
            project.write_json("contracts/transitions/zone-transition.manifest.json", &zone);
            let value = project.read_json(".appsdk/project.json");
            assert_eq!(
                try_assert_declared_contracts(&project.root, &value).unwrap_err(),
                expected
            );
        }
        let project = TestProject::new("transition-noncanonical");
        let mut value = project.read_json(".appsdk/project.json");
        value["governance"]["zone_transition_contract"] =
            json!("contracts/transitions/../zone-transition.manifest.json");
        assert_eq!(
            try_assert_declared_contracts(&project.root, &value).unwrap_err(),
            "INVALID_GOVERNANCE_ROOT:/governance/zone_transition_contract"
        );
    }

    #[test]
    fn registry_binding_and_module_local_contracts_are_explicit() {
        assert!(matches!(
            try_assert_registry_binding_contract(&json!({}), "app-core").unwrap(),
            RegistryBinding::Exact
        ));
        assert!(matches!(
            try_assert_registry_binding_contract(
                &json!({"registry_binding":{"mode":"exact"}}),
                "app-core"
            )
            .unwrap(),
            RegistryBinding::Exact
        ));
        assert!(matches!(
            try_assert_registry_binding_contract(
                &json!({"registry_binding":{"mode":"aggregate","modules":["a","b"]}}),
                "app-core"
            )
            .unwrap(),
            RegistryBinding::Aggregate(_)
        ));
        for binding in [
            json!({"mode":"exact","modules":[]}),
            json!({"mode":"exact","extra":true}),
            json!({"mode":"aggregate","modules":[]}),
            json!({"mode":"aggregate","modules":["a","a"]}),
            json!({"mode":"aggregate","modules":[""]}),
            json!({"mode":"aggregate","modules":[1]}),
        ] {
            match try_assert_registry_binding_contract(
                &json!({"registry_binding":binding}),
                "app-core",
            ) {
                Err(error) => assert_eq!(error, "INVALID_REGISTRY_BINDING:app-core"),
                Ok(_) => panic!("registry binding unexpectedly succeeded: {binding}"),
            }
        }

        assert_eq!(
            try_module_deployment_operations(&json!({})).unwrap(),
            vec!["install", "restart"]
        );
        assert_eq!(
            try_module_deployment_operations(&json!({"deployment_operations":["install"]}))
                .unwrap(),
            vec!["install"]
        );
        for operations in [
            json!("install"),
            json!(["deploy"]),
            json!(["install", "install"]),
        ] {
            assert_eq!(
                try_module_deployment_operations(&json!({"deployment_operations":operations}))
                    .unwrap_err(),
                "INVALID_DEPLOYMENT_OPERATIONS"
            );
        }
        assert_eq!(
            try_assert_module_regression_contract(&json!({}), "draft", "app-core"),
            Ok(())
        );
        assert_eq!(
            try_assert_module_regression_contract(&json!({}), "frozen", "app-core"),
            Err("REGRESSION_CONTRACT_REQUIRED:app-core".to_owned())
        );
        let regression = TestProject::new("regression-valid");
        let valid_module = regression.read_json(".appsdk/project.json")["modules"][0].clone();
        assert_eq!(
            try_assert_module_regression_contract(&valid_module, "source_implemented", "app-core"),
            Ok(())
        );
        let mut disabled_at_freeze = valid_module.clone();
        disabled_at_freeze["regression"]["required_before_freeze"] = json!(false);
        assert_eq!(
            try_assert_module_regression_contract(&disabled_at_freeze, "frozen", "app-core"),
            Err("INVALID_REGRESSION_CONTRACT:app-core".to_owned())
        );
        let mut bad = valid_module.clone();
        bad["regression"]["command"]["args"] = json!([1]);
        assert_eq!(
            try_assert_module_regression_contract(&bad, "source_implemented", "app-core"),
            Err("INVALID_REGRESSION_CONTRACT:app-core".to_owned())
        );
    }

    #[test]
    fn project_module_surfaces_stages_dependencies_and_build_fields_are_checked() {
        let cases: [(&str, fn(&mut Value), &'static str); 9] = [
            (
                "bad-stage",
                |p| p["modules"][0]["stage"] = json!("unknown"),
                "INVALID_PROJECT_MODULE:app-core",
            ),
            (
                "bad-owner",
                |p| p["modules"][0]["source_owner"] = json!("other"),
                "INVALID_PROJECT_MODULE:app-core",
            ),
            (
                "bad-surface",
                |p| p["modules"][0]["owned_paths"] = json!([]),
                "INVALID_PROJECT_MODULE:app-core",
            ),
            (
                "bad-dependency-type",
                |p| p["modules"][0]["dependency_modules"] = json!([1]),
                "INVALID_PROJECT_MODULE:app-core",
            ),
            (
                "missing-dependency",
                |p| p["modules"][0]["dependency_modules"] = json!(["missing"]),
                "INVALID_PROJECT_MODULE:app-core:dependency",
            ),
            (
                "bad-build",
                |p| p["modules"][0]["build"]["program"] = json!(""),
                "INVALID_PROJECT_MODULE:app-core:build/program",
            ),
            (
                "duplicate-id",
                |p| p["modules"] = json!([p["modules"][0].clone(), p["modules"][0].clone()]),
                "INVALID_PROJECT_MODULE:app-core",
            ),
            (
                "overlap",
                |p| {
                    let mut second = p["modules"][0].clone();
                    second["module_id"] = json!("other");
                    second["source_owner"] = json!("other");
                    second["active_artifact"] = json!("active/lib/other/**");
                    p["modules"] = json!([p["modules"][0].clone(), second]);
                },
                "OVERLAPPING_MODULE_OWNERSHIP",
            ),
            (
                "version-base-equal",
                |p| p["modules"][0]["version_base"] = json!({"previous_active_version":"v1","new_active_version":"v1","base_artifact_hash":"h","base_source_commit":"c"}),
                "INVALID_MODULE_VERSION_BASE:app-core",
            ),
        ];
        for (label, mutate, expected) in cases {
            let project = TestProject::new(label);
            let mut value = project.read_json(".appsdk/project.json");
            mutate(&mut value);
            assert_eq!(
                try_assert_project_contract(&project.root, &value).unwrap_err(),
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn reset_record_accepts_only_transactional_receipts_without_git_lookup() {
        let valid = json!({
            "schema_version": 1,
            "mode": "discard_legacy_control_plane",
            "reset_id": "tx-123",
            "transaction_id": "tx-123",
            "branch": "codex/reset",
            "created_at": "2020-01-01T00:00:00Z",
            "preserved": ["protected"],
            "removed": ["legacy"]
        });
        let project = TestProject::new("transactional-reset");
        assert_eq!(
            try_reset_governance_record_mode(&project.root).unwrap(),
            None
        );
        project.write_json(".appsdk/records/reset-governance-record.json", &valid);
        assert_eq!(
            try_reset_governance_record_mode(&project.root).unwrap(),
            Some("discard_legacy_control_plane".to_owned())
        );
        let mut fresh_init = valid.clone();
        fresh_init["mode"] = json!("fresh_init");
        project.write_json(".appsdk/records/reset-governance-record.json", &fresh_init);
        assert_eq!(
            try_reset_governance_record_mode(&project.root).unwrap(),
            Some("fresh_init".to_owned())
        );

        let cases: [(&str, Value); 10] = [
            ("schema", json!({"schema_version":2})),
            ("mode", json!({"mode":"legacy"})),
            ("transaction", json!({"reset_id":"a","transaction_id":"b"})),
            ("timestamp", json!({"created_at":"not-a-date"})),
            ("future", json!({"created_at":"2999-01-01T00:00:00Z"})),
            ("arrays", json!({"preserved":[]})),
            ("array-member", json!({"removed":[""]})),
            ("invalid-json", json!({})),
            ("missing-reset-id", json!({"reset_id":null})),
            ("missing-branch", json!({"branch":null})),
        ];
        for (label, patch) in cases {
            let project = TestProject::new(label);
            let mut record = valid.clone();
            for (key, value) in patch.as_object().unwrap() {
                record[key] = value.clone();
            }
            if label == "invalid-json" {
                fs::write(
                    project.path(".appsdk/records/reset-governance-record.json"),
                    "{",
                )
                .unwrap();
            } else {
                project.write_json(".appsdk/records/reset-governance-record.json", &record);
            }
            assert_eq!(
                try_reset_governance_record_mode(&project.root).unwrap_err(),
                "INVALID_RESET_GOVERNANCE_RECORD",
                "{label}"
            );
        }

        let project = TestProject::new("reset-record-directory");
        fs::create_dir_all(project.path(".appsdk/records/reset-governance-record.json")).unwrap();
        assert_eq!(
            try_reset_governance_record_mode(&project.root).unwrap_err(),
            "INVALID_RESET_GOVERNANCE_RECORD"
        );

        #[cfg(unix)]
        {
            let project = TestProject::new("reset-record-symlink");
            let target = project.path("reset-record-target.json");
            fs::write(&target, serde_json::to_vec(&valid).unwrap()).unwrap();
            std::os::unix::fs::symlink(
                target,
                project.path(".appsdk/records/reset-governance-record.json"),
            )
            .unwrap();
            assert_eq!(
                try_reset_governance_record_mode(&project.root).unwrap_err(),
                "GOVERNANCE_PATH_SYMLINK:reset_governance_record"
            );
        }
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

        let invocation = Arc::new(VerifyInvocation::new(project.root.clone()).unwrap());
        assert!(invocation.reset_epoch);
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
            VerifyInvocation::new(missing).unwrap_err(),
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
            VerifyInvocation::new(project.root.clone()).unwrap_err(),
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
