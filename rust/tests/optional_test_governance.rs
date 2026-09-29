use serde_json::Value;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_appsdk"))
}

fn test_global_registry_root_for_args(args: &[&str]) -> PathBuf {
    args.iter()
        .skip(1)
        .map(Path::new)
        .find(|path| path.is_absolute())
        .map(|root| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            root.to_string_lossy().hash(&mut hasher);
            std::env::temp_dir().join(format!(
                "appsdk-rust-global-registry-tests-{}-{:016x}",
                std::process::id(),
                hasher.finish()
            ))
        })
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "appsdk-rust-global-registry-tests-{}-default",
                std::process::id()
            ))
        })
}

fn temp_root(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("appsdk-rust-{name}-{}-{nonce}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    root
}

fn test_git_bug_fixture_for_root(root: &Path) -> Option<PathBuf> {
    let candidate = root.with_extension("fake-git-bug").join("git-bug");
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

fn apply_test_git_bug_fixture(command: &mut Command, root: &Path) {
    if let Some(path) = test_git_bug_fixture_for_root(root) {
        command.env("GIT_BUG_BIN", path);
    } else {
        command.env_remove("GIT_BUG_BIN");
    }
}

fn run(args: &[&str]) -> Output {
    let mut command = Command::new(binary());
    command
        .args(args)
        .env("APPSDK_HOME", test_global_registry_root_for_args(args))
        .env_remove("TMUX_PANE");
    if let Some(root) = args
        .iter()
        .skip(1)
        .map(Path::new)
        .find(|path| path.is_absolute())
    {
        apply_test_git_bug_fixture(&mut command, root);
    } else {
        command.env_remove("GIT_BUG_BIN");
    }
    command.output().unwrap()
}

fn init_git(root: &PathBuf) {
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "init"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "config",
            "maintenance.auto",
            "false"
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "config",
            "user.email",
            "test@appsdk.local"
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "config",
            "user.name",
            "AppSDK Test"
        ])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "commit", "-m", "baseline"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root.to_str().unwrap(), "branch", "-M", "codex/test"])
        .status()
        .unwrap()
        .success());
}

#[test]
fn optional_test_governance_off_is_compatible_and_compile_does_not_depend_on_test_manifest() {
    let root = temp_root("optional-test-governance-off");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let test_admission = run(&["verify", "--test-admission", root_text]);
    assert!(
        test_admission.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&test_admission.stdout),
        String::from_utf8_lossy(&test_admission.stderr)
    );
    let report: Value = serde_json::from_slice(&test_admission.stdout).unwrap();
    assert_eq!(report["mode"], "off");
    assert_eq!(report["status"], "not_selected");

    let admission = run(&["verify", "--admission", root_text]);
    assert!(
        admission.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&admission.stdout),
        String::from_utf8_lossy(&admission.stderr)
    );

    let goal = root.join(".appsdk/goal.json");
    let mut goal_value: Value = serde_json::from_slice(&fs::read(&goal).unwrap()).unwrap();
    goal_value["status"] = Value::String("confirmed".into());
    goal_value["confirmed_by"] = Value::String("test".into());
    goal_value["confirmed_at"] = Value::String("2026-01-01T00:00:00Z".into());
    fs::write(&goal, serde_json::to_vec_pretty(&goal_value).unwrap()).unwrap();

    let project = root.join(".appsdk/project.json");
    let mut project_value: Value = serde_json::from_slice(&fs::read(&project).unwrap()).unwrap();
    project_value["test_governance"] = serde_json::json!({
        "mode": "selected",
        "manifest": ".appsdk/test-governance.json"
    });
    fs::write(
        &project,
        serde_json::to_string_pretty(&project_value).unwrap() + "\n",
    )
    .unwrap();

    init_git(&root);
    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    let compiled = run(&["compile", root_text]);
    assert!(
        compiled.status.success(),
        "compile must not depend on the optional test manifest; stdout={} stderr={}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn optional_test_governance_selection_requires_scope_semantic_graph_and_scenarios() {
    let root = temp_root("optional-test-governance-required-fields");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let project = root.join(".appsdk/project.json");
    let mut project_value: Value = serde_json::from_slice(&fs::read(&project).unwrap()).unwrap();
    project_value["test_governance"] = serde_json::json!({
        "mode": "selected",
        "manifest": ".appsdk/test-governance.json"
    });
    fs::write(
        &project,
        serde_json::to_string_pretty(&project_value).unwrap() + "\n",
    )
    .unwrap();

    let manifest = root.join(".appsdk/test-governance.json");
    fs::write(&manifest, r#"{"schema_version":1,"mode":"selected","objects":[],"trusted_runners":[],"effect_authorizations":[]}"#.to_owned() + "\n")
        .unwrap();
    let rejected = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("TEST_GOVERNANCE_OBJECTS_REQUIRED"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    fs::write(&manifest, r#"{"schema_version":1,"mode":"selected","objects":[{"object_id":"app-core","graph_id":"app-core","graph_version":"1","scope_confirmation":{"reference":"ref","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z"},"scenarios":[]}],"trusted_runners":[{"runner_ref":"runner","entrypoint":"entry","owner":"app-core"}],"effect_authorizations":[]}"#.to_owned() + "\n")
        .unwrap();
    let rejected_missing_graph = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected_missing_graph.status.success());
    assert!(
        String::from_utf8_lossy(&rejected_missing_graph.stderr)
            .contains("TEST_GOVERNANCE_SEMANTIC_GRAPH_REQUIRED"),
        "stderr={}",
        String::from_utf8_lossy(&rejected_missing_graph.stderr)
    );

    fs::write(&manifest, r#"{"schema_version":1,"mode":"selected","objects":[{"object_id":"app-core","graph_id":"app-core","graph_version":"1","semantic_graph":{"entry":"trigger","exit":"terminal","nodes":[{"id":"trigger","label":"收到外部订单请求"},{"id":"terminal","label":"完成验收并交付结果"}],"edges":[{"from":"trigger","to":"terminal"}]},"scope_confirmation":{"reference":"ref","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z"},"scenarios":[]}],"trusted_runners":[{"runner_ref":"runner","entrypoint":"entry","owner":"app-core"}],"effect_authorizations":[]}"#.to_owned() + "\n")
        .unwrap();
    let rejected_empty = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected_empty.status.success());
    assert!(
        String::from_utf8_lossy(&rejected_empty.stderr)
            .contains("TEST_GOVERNANCE_SCENARIOS_REQUIRED"),
        "stderr={}",
        String::from_utf8_lossy(&rejected_empty.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn optional_test_governance_rejects_invalid_semantic_graphs_and_unmapped_scenarios() {
    let root = temp_root("optional-test-governance-semantic-graph-rejection");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    let project = root.join(".appsdk/project.json");
    let mut project_value: Value = serde_json::from_slice(&fs::read(&project).unwrap()).unwrap();
    project_value["test_governance"] = serde_json::json!({
        "mode": "selected",
        "manifest": ".appsdk/test-governance.json"
    });
    fs::write(
        &project,
        serde_json::to_string_pretty(&project_value).unwrap() + "\n",
    )
    .unwrap();

    let manifest_path = root.join(".appsdk/test-governance.json");
    fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    let schema_manifest = serde_json::json!({
        "schema_version": 1,
        "mode": "selected",
        "objects": [{
            "object_id": "app-core",
            "graph_id": "app-core",
            "graph_version": "1",
            "semantic_graph": {
                "entry": "trigger",
                "exit": "terminal",
                "nodes": [
                    {"id": "trigger", "label": "收到外部订单请求"},
                    {"id": "terminal", "label": "完成验收并交付结果"}
                ],
                "edges": [
                    {"from": "trigger", "to": "terminal"}
                ]
            },
            "scope_confirmation": {
                "reference": "evidence://scope/app-core",
                "confirmed_by": "test",
                "confirmed_at": "2026-01-01T00:00:00Z"
            },
            "scenarios": [{
                "scenario_id": "scenario-1",
                "semantic_name": "happy path",
                "entrypoint": "POST /orders",
                "preconditions": ["isolated fixture"],
                "stimulus": "submit order",
                "observable_assertions": ["returns accepted"],
                "path_node_ids": ["trigger", "terminal"],
                "expected_effects": ["order created"],
                "cleanup": "remove fixture",
                "runner_ref": "runner-1",
                "classification": ["normal"]
            }]
        }],
        "trusted_runners": [{
            "runner_ref": "runner-1",
            "entrypoint": "POST /orders",
            "owner": "app-core"
        }],
        "effect_authorizations": []
    });
    let write_manifest = |manifest: &Value| {
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(manifest).unwrap() + "\n",
        )
        .unwrap();
    };
    write_manifest(&schema_manifest);
    let accepted = run(&["verify", "--test-admission", root_text]);
    let accepted_stderr = String::from_utf8_lossy(&accepted.stderr);
    assert!(
        !accepted_stderr.contains("TEST_GOVERNANCE_SEMANTIC_GRAPH")
            && !accepted_stderr.contains("INVALID_TEST_GOVERNANCE_MANIFEST"),
        "valid semantic graph must pass manifest validation; stderr={}",
        accepted_stderr
    );
    let accepted_report: Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(accepted_report["objects"][0]["status"], "blocked");

    let mut missing_graph = schema_manifest.clone();
    missing_graph["objects"][0]
        .as_object_mut()
        .unwrap()
        .remove("semantic_graph");
    write_manifest(&missing_graph);
    let rejected = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("TEST_GOVERNANCE_SEMANTIC_GRAPH_REQUIRED"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    let mut non_chinese = schema_manifest.clone();
    non_chinese["objects"][0]["semantic_graph"]["nodes"][0]["label"] =
        Value::String("submit order".into());
    write_manifest(&non_chinese);
    let rejected = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("TEST_GOVERNANCE_SEMANTIC_GRAPH_NODE_LABEL_NOT_CHINESE"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    let mut multi_entry = schema_manifest.clone();
    multi_entry["objects"][0]["semantic_graph"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "second-source",
            "label": "第二个外部触发"
        }));
    multi_entry["objects"][0]["semantic_graph"]["edges"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "from": "second-source",
            "to": "terminal"
        }));
    write_manifest(&multi_entry);
    let rejected = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("TEST_GOVERNANCE_SEMANTIC_GRAPH_NOT_SESE_MULTI_ENTRY"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    let mut multi_exit = schema_manifest.clone();
    multi_exit["objects"][0]["semantic_graph"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "second-exit",
            "label": "另一个接受退出"
        }));
    multi_exit["objects"][0]["semantic_graph"]["edges"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "from": "trigger",
            "to": "second-exit"
        }));
    write_manifest(&multi_exit);
    let rejected = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("TEST_GOVERNANCE_SEMANTIC_GRAPH_NOT_SESE_MULTI_EXIT"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    let mut unmapped = schema_manifest.clone();
    unmapped["objects"][0]["scenarios"][0]["path_node_ids"] =
        serde_json::json!(["trigger", "missing-node"]);
    write_manifest(&unmapped);
    let rejected = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("TEST_GOVERNANCE_SCENARIO_PATH_NODE_UNDEFINED"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    let schema: Value = serde_json::from_slice(include_bytes!(
        "../../contracts/test-governance.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(!validator.is_valid(&missing_graph));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn optional_test_governance_runner_grammar_matches_contract_and_schema() {
    let root = temp_root("optional-test-governance-runner-grammar");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    let project = root.join(".appsdk/project.json");
    let mut project_value: Value = serde_json::from_slice(&fs::read(&project).unwrap()).unwrap();
    project_value["test_governance"] = serde_json::json!({
        "mode": "selected",
        "manifest": ".appsdk/test-governance.json"
    });
    fs::write(
        &project,
        serde_json::to_string_pretty(&project_value).unwrap() + "\n",
    )
    .unwrap();

    let manifest_path = root.join(".appsdk/test-governance.json");
    fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    let schema_manifest = serde_json::json!({
        "schema_version": 1,
        "mode": "selected",
        "objects": [{
            "object_id": "app-core",
            "graph_id": "app-core",
            "graph_version": "1",
            "semantic_graph": {
                "entry": "trigger",
                "exit": "terminal",
                "nodes": [
                    {"id": "trigger", "label": "收到外部订单请求"},
                    {"id": "terminal", "label": "完成验收并交付结果"}
                ],
                "edges": [
                    {"from": "trigger", "to": "terminal"}
                ]
            },
            "scope_confirmation": {
                "reference": "evidence://scope/app-core",
                "confirmed_by": "test",
                "confirmed_at": "2026-01-01T00:00:00Z"
            },
            "scenarios": [{
                "scenario_id": "scenario-1",
                "semantic_name": "happy path",
                "entrypoint": "POST /orders",
                "preconditions": ["isolated fixture"],
                "stimulus": "submit order",
                "observable_assertions": ["returns accepted"],
                "path_node_ids": ["trigger", "terminal"],
                "expected_effects": ["order created"],
                "cleanup": "remove fixture",
                "runner_ref": "runner-ref",
                "classification": ["normal"]
            }]
        }],
        "trusted_runners": [{
            "runner_ref": "runner-ref",
            "entrypoint": "POST /orders",
            "owner": "app-core"
        }],
        "effect_authorizations": []
    });
    let write_manifest = |runner: &str| {
        let mut manifest = schema_manifest.clone();
        manifest["trusted_runners"][0]["runner_ref"] = Value::String(runner.to_string());
        manifest["objects"][0]["scenarios"][0]["runner_ref"] = Value::String(runner.to_string());
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap() + "\n",
        )
        .unwrap();
    };

    write_manifest("runner-ref");
    let accepted = run(&["verify", "--test-admission", root_text]);
    assert!(
        !String::from_utf8_lossy(&accepted.stderr).contains("INVALID_TEST_GOVERNANCE_RUNNER"),
        "hyphenated runner_ref must pass runtime validation; stderr={}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let accepted_report: Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(accepted_report["objects"][0]["status"], "blocked");
    assert_eq!(
        accepted_report["objects"][0]["scenarios"][0]["reason"],
        "result_missing"
    );

    for invalid_runner in ["runner.one", "r"] {
        write_manifest(invalid_runner);
        let rejected = run(&["verify", "--test-admission", root_text]);
        assert!(
            !rejected.status.success(),
            "{invalid_runner} must be rejected as a runner_ref"
        );
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("INVALID_TEST_GOVERNANCE_RUNNER"),
            "{invalid_runner} must fail runtime identifier grammar; stderr={}",
            String::from_utf8_lossy(&rejected.stderr)
        );
    }

    let schema: Value = serde_json::from_slice(include_bytes!(
        "../../contracts/test-governance.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&schema_manifest));
    let mut dot_manifest = schema_manifest.clone();
    dot_manifest["trusted_runners"][0]["runner_ref"] = Value::String("runner.one".into());
    assert!(!validator.is_valid(&dot_manifest));
    let mut one_char_manifest = schema_manifest.clone();
    one_char_manifest["trusted_runners"][0]["runner_ref"] = Value::String("r".into());
    assert!(!validator.is_valid(&one_char_manifest));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn optional_test_governance_effect_requires_authorization_and_passed_evidence_closes() {
    let root = temp_root("optional-test-governance-admission");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    let head = Command::new("git")
        .args(["-C", root_text, "rev-parse", "HEAD"])
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();

    let project = root.join(".appsdk/project.json");
    let mut project_value: Value = serde_json::from_slice(&fs::read(&project).unwrap()).unwrap();
    project_value["test_governance"] = serde_json::json!({
        "mode": "selected",
        "manifest": ".appsdk/test-governance.json"
    });
    fs::write(
        &project,
        serde_json::to_string_pretty(&project_value).unwrap() + "\n",
    )
    .unwrap();

    let manifest = root.join(".appsdk/test-governance.json");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(
        &manifest,
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "mode": "selected",
            "objects": [{
                "object_id": "app-core",
                "graph_id": "app-core",
                "graph_version": "1",
                "invariants": [{
                    "invariant_id": "total-balance-zero",
                    "semantic_name": "所有账户余额总和必须为零",
                    "applies_to_scenarios": ["scenario-1"]
                }],
                "semantic_graph": {
                    "entry": "trigger",
                    "exit": "terminal",
                    "nodes": [
                        {"id": "trigger", "label": "收到外部订单请求"},
                        {"id": "terminal", "label": "完成验收并交付结果"}
                    ],
                    "edges": [
                        {"from": "trigger", "to": "terminal"}
                    ]
                },
                "scope_confirmation": {
                    "reference": "evidence://scope/app-core",
                    "confirmed_by": "test",
                    "confirmed_at": "2026-01-01T00:00:00Z"
                },
                "scenarios": [{
                    "scenario_id": "scenario-1",
                    "semantic_name": "happy path",
                    "entrypoint": "POST /orders",
                    "preconditions": ["isolated fixture"],
                    "stimulus": "submit order",
                    "observable_assertions": ["returns accepted"],
                    "path_node_ids": ["trigger", "terminal"],
                    "expected_effects": ["order created"],
                    "cleanup": "remove fixture",
                    "runner_ref": "runner-1",
                    "classification": ["normal"]
                }]
            }],
            "trusted_runners": [{
                "runner_ref": "runner-1",
                "entrypoint": "POST /orders",
                "owner": "app-core"
            }],
            "effect_authorizations": []
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();

    let blocked_no_evidence = run(&["verify", "--test-admission", root_text]);
    assert!(!blocked_no_evidence.status.success());
    let report: Value = serde_json::from_slice(&blocked_no_evidence.stdout).unwrap();
    assert_eq!(report["status"], "blocked");
    assert_eq!(report["objects"][0]["status"], "blocked");
    assert_eq!(report["objects"][0]["invariants"][0]["status"], "blocked");
    assert_eq!(
        report["objects"][0]["invariants"][0]["reason"],
        "invariant_not_covered"
    );
    let admission = run(&["verify", "--admission", root_text]);
    assert!(!admission.status.success());
    assert!(String::from_utf8_lossy(&admission.stderr).contains("TEST_GOVERNANCE_BLOCKED"));

    let evidence_dir = root.join(".appsdk/records/evidence/app-core");
    fs::create_dir_all(&evidence_dir).unwrap();
    fs::write(
        evidence_dir.join("evidence-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "evidence_id": "evidence-1",
            "issue_id": "4995b1a",
            "experiment_id": "optional-governance",
            "phase": "deployed_blackbox",
            "kind": "sample_replay",
            "artifact_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "source_commit": head,
            "execution_surface": "deployed_blackbox",
            "environment_id": "test",
            "entrypoint": "POST /orders",
            "scope": {"module_id": "app-core"},
            "producer": {"adapter": "test", "identity": "test-worker"},
            "result": "pass",
            "created_at": "2026-01-02T00:00:00Z",
            "expires_at": "2099-01-01T00:00:00Z",
            "input_hashes": [],
            "scope_hash": "abc"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();

    let result_dir = root.join(".appsdk/records/test-scenario-results/app-core");
    fs::create_dir_all(&result_dir).unwrap();
    fs::write(
        result_dir.join("scenario-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "result_id": "result-1",
            "object_id": "app-core",
            "scenario_id": "scenario-1",
            "graph_id": "app-core",
            "graph_version": "1",
            "candidate_commit": head,
            "status": "passed",
            "entrypoint": "POST /orders",
            "environment": "test",
            "evidence_id": "evidence-1",
            "cleanup_result": {"status": "passed", "detail": "cleaned"},
            "producer": {"adapter": "test", "identity": "test-worker"},
            "started_at": "2026-01-02T00:00:00Z",
            "finished_at": "2026-01-02T00:00:05Z"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();

    let passed = run(&["verify", "--test-admission", root_text]);
    assert!(
        passed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&passed.stdout),
        String::from_utf8_lossy(&passed.stderr)
    );
    let passed_report: Value = serde_json::from_slice(&passed.stdout).unwrap();
    assert_eq!(passed_report["objects"][0]["status"], "passed");
    assert_eq!(
        passed_report["objects"][0]["invariants"][0]["status"],
        "covered"
    );
    assert_eq!(
        passed_report["objects"][0]["invariants"][0]["covered_by"],
        serde_json::json!(["scenario-1"])
    );

    let admission = run(&["verify", "--admission", root_text]);
    assert!(
        admission.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&admission.stdout),
        String::from_utf8_lossy(&admission.stderr)
    );

    let unselected = run(&[
        "verify",
        "--test-admission",
        root_text,
        "--object",
        "unselected",
    ]);
    assert!(unselected.status.success());
    let unselected_report: Value = serde_json::from_slice(&unselected.stdout).unwrap();
    assert_eq!(unselected_report["objects"][0]["status"], "not_selected");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn optional_test_governance_rejects_invalid_invariants() {
    let root = temp_root("optional-test-governance-invalid-invariants");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    let project = root.join(".appsdk/project.json");
    let mut project_value: Value = serde_json::from_slice(&fs::read(&project).unwrap()).unwrap();
    project_value["test_governance"] = serde_json::json!({
        "mode": "selected",
        "manifest": ".appsdk/test-governance.json"
    });
    fs::write(
        &project,
        serde_json::to_string_pretty(&project_value).unwrap() + "\n",
    )
    .unwrap();
    let manifest_path = root.join(".appsdk/test-governance.json");
    let write_manifest = |invariants: &Value| {
        fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "mode": "selected",
                "objects": [{
                    "object_id": "app-core",
                    "graph_id": "app-core",
                    "graph_version": "1",
                    "invariants": invariants,
                    "semantic_graph": {
                        "entry": "trigger",
                        "exit": "terminal",
                        "nodes": [
                            {"id": "trigger", "label": "收到外部订单请求"},
                            {"id": "terminal", "label": "完成验收并交付结果"}
                        ],
                        "edges": [{"from": "trigger", "to": "terminal"}]
                    },
                    "scope_confirmation": {
                        "reference": "evidence://scope/app-core",
                        "confirmed_by": "test",
                        "confirmed_at": "2026-01-01T00:00:00Z"
                    },
                    "scenarios": [{
                        "scenario_id": "scenario-1",
                        "semantic_name": "happy path",
                        "entrypoint": "POST /orders",
                        "preconditions": ["isolated fixture"],
                        "stimulus": "submit order",
                        "observable_assertions": ["returns accepted"],
                        "path_node_ids": ["trigger", "terminal"],
                        "expected_effects": ["order created"],
                        "cleanup": "remove fixture",
                        "runner_ref": "runner-1",
                        "classification": ["normal"]
                    }]
                }],
                "trusted_runners": [{
                    "runner_ref": "runner-1",
                    "entrypoint": "POST /orders",
                    "owner": "app-core"
                }],
                "effect_authorizations": []
            }))
            .unwrap()
                + "\n",
        )
        .unwrap();
    };

    let duplicate = serde_json::json!([
        {
            "invariant_id": "total-balance-zero",
            "semantic_name": "所有账户余额总和必须为零",
            "applies_to_scenarios": ["scenario-1"]
        },
        {
            "invariant_id": "total-balance-zero",
            "semantic_name": "另一个对象级约束",
            "applies_to_scenarios": ["scenario-1"]
        }
    ]);
    write_manifest(&duplicate);
    let rejected = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("DUPLICATE_TEST_GOVERNANCE_INVARIANT"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    let empty_refs = serde_json::json!([
        {
            "invariant_id": "total-balance-zero",
            "semantic_name": "所有账户余额总和必须为零",
            "applies_to_scenarios": []
        }
    ]);
    write_manifest(&empty_refs);
    let rejected = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("TEST_GOVERNANCE_INVARIANT_SCENARIOS_REQUIRED"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    let missing_scenario = serde_json::json!([
        {
            "invariant_id": "total-balance-zero",
            "semantic_name": "所有账户余额总和必须为零",
            "applies_to_scenarios": ["missing-scenario"]
        }
    ]);
    write_manifest(&missing_scenario);
    let rejected = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("TEST_GOVERNANCE_INVARIANT_SCENARIO_NOT_FOUND"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn optional_test_governance_blocks_bad_results_effect_and_evidence_mismatches() {
    let root = temp_root("optional-test-governance-blockers");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    let head = Command::new("git")
        .args(["-C", root_text, "rev-parse", "HEAD"])
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();

    let project = root.join(".appsdk/project.json");
    let mut project_value: Value = serde_json::from_slice(&fs::read(&project).unwrap()).unwrap();
    project_value["test_governance"] = serde_json::json!({
        "mode": "selected",
        "manifest": ".appsdk/test-governance.json"
    });
    fs::write(
        &project,
        serde_json::to_string_pretty(&project_value).unwrap() + "\n",
    )
    .unwrap();

    let manifest_path = root.join(".appsdk/test-governance.json");
    fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
    let manifest = serde_json::json!({
        "schema_version": 1,
        "mode": "selected",
        "objects": [{
            "object_id": "app-core",
            "graph_id": "app-core",
            "graph_version": "1",
            "semantic_graph": {
                "entry": "trigger",
                "exit": "terminal",
                "nodes": [
                    {"id": "trigger", "label": "收到外部订单请求"},
                    {"id": "terminal", "label": "完成验收并交付结果"}
                ],
                "edges": [
                    {"from": "trigger", "to": "terminal"}
                ]
            },
            "scope_confirmation": {
                "reference": "evidence://scope/app-core",
                "confirmed_by": "test",
                "confirmed_at": "2026-01-01T00:00:00Z"
            },
            "scenarios": [{
                "scenario_id": "scenario-1",
                "semantic_name": "effect path",
                "entrypoint": "POST /orders",
                "preconditions": ["isolated fixture"],
                "stimulus": "submit order",
                "observable_assertions": ["returns accepted"],
                "path_node_ids": ["trigger", "terminal"],
                "expected_effects": ["order created"],
                "cleanup": "remove fixture",
                "runner_ref": "runner-1",
                "classification": ["effect"],
                "effect_authorization_id": "auth-1"
            }]
        }],
        "trusted_runners": [{
            "runner_ref": "runner-1",
            "entrypoint": "POST /orders",
            "owner": "app-core"
        }],
        "effect_authorizations": [{
            "authorization_id": "auth-1",
            "object_id": "app-core",
            "scenario_id": "scenario-1",
            "environment": "test",
            "allowed_effects": ["order created"],
            "valid_from": "2026-01-01T00:00:00Z",
            "valid_until": "2099-01-01T00:00:00Z",
            "approval_ref": "approval://owners/app-core"
        }]
    });
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap() + "\n",
    )
    .unwrap();

    let evidence_dir = root.join(".appsdk/records/evidence/app-core");
    fs::create_dir_all(&evidence_dir).unwrap();
    fs::write(
        evidence_dir.join("evidence-1.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "evidence_id": "evidence-1",
            "issue_id": "4995b1a",
            "experiment_id": "optional-governance",
            "phase": "deployed_blackbox",
            "kind": "sample_replay",
            "artifact_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "source_commit": head,
            "execution_surface": "deployed_blackbox",
            "environment_id": "test",
            "entrypoint": "POST /orders",
            "scope": {"module_id": "app-core"},
            "producer": {"adapter": "test", "identity": "test-worker"},
            "result": "pass",
            "created_at": "2026-01-02T00:00:00Z",
            "expires_at": "2099-01-01T00:00:00Z",
            "input_hashes": [],
            "scope_hash": "abc"
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();

    let result_dir = root.join(".appsdk/records/test-scenario-results/app-core");
    fs::create_dir_all(&result_dir).unwrap();
    let result_path = result_dir.join("scenario-1.json");
    let write_result = |status: &str,
                        effect_auth: &str,
                        graph_version: &str,
                        candidate: &str,
                        scenario_id: &str,
                        cleanup_status: &str| {
        fs::write(
            &result_path,
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "result_id": "result-1",
                "object_id": "app-core",
                "scenario_id": scenario_id,
                "graph_id": "app-core",
                "graph_version": graph_version,
                "candidate_commit": candidate,
                "status": status,
                "entrypoint": "POST /orders",
                "environment": "test",
                "evidence_id": if status == "passed" {"evidence-1"} else {""},
                "effect_authorization_id": effect_auth,
                "cleanup_result": {"status": cleanup_status, "detail": "cleaned"},
                "producer": {"adapter": "test", "identity": "test-worker"},
                "started_at": "2026-01-02T00:00:00Z",
                "finished_at": "2026-01-02T00:00:05Z"
            }))
            .unwrap()
                + "\n",
        )
        .unwrap();
    };

    let cases: [(&str, &str, &str, &str, &str, &str); 8] = [
        ("wrong-graph", "auth-1", "2", &head, "scenario-1", "passed"),
        (
            "wrong-commit",
            "auth-1",
            "1",
            "not-the-candidate",
            "scenario-1",
            "passed",
        ),
        (
            "wrong-scenario",
            "auth-1",
            "1",
            &head,
            "scenario-other",
            "passed",
        ),
        (
            "missing-auth",
            "auth-missing",
            "1",
            &head,
            "scenario-1",
            "passed",
        ),
        ("planned", "auth-1", "1", &head, "scenario-1", "passed"),
        ("failed", "auth-1", "1", &head, "scenario-1", "passed"),
        ("blocked", "auth-1", "1", &head, "scenario-1", "passed"),
        (
            "cleanup-failed",
            "auth-1",
            "1",
            &head,
            "scenario-1",
            "failed",
        ),
    ];
    for (label, effect_auth, graph_version, candidate, scenario_id, cleanup_status) in cases {
        let status = match label {
            "planned" => "planned",
            "failed" => "failed",
            "blocked" => "blocked",
            _ => "passed",
        };
        write_result(
            status,
            effect_auth,
            graph_version,
            candidate,
            scenario_id,
            cleanup_status,
        );
        let rejected = run(&["verify", "--test-admission", root_text]);
        assert!(
            !rejected.status.success(),
            "{label} must block optional test governance"
        );
        let report: Value = serde_json::from_slice(&rejected.stdout).unwrap();
        assert_eq!(
            report["objects"][0]["status"],
            "blocked",
            "{label}: {report}",
            label = label
        );
    }

    let ordinary_verify = run(&["verify", root_text]);
    assert!(
        ordinary_verify.status.success(),
        "ordinary verify must report blocked optional test governance without requiring tests to pass; stdout={} stderr={}",
        String::from_utf8_lossy(&ordinary_verify.stdout),
        String::from_utf8_lossy(&ordinary_verify.stderr)
    );
    let ordinary_report: Value = serde_json::from_slice(&ordinary_verify.stdout).unwrap();
    assert_eq!(ordinary_report["test_governance"]["status"], "blocked");

    write_result("passed", "auth-1", "1", &head, "scenario-1", "passed");

    let evidence_path = evidence_dir.join("evidence-1.json");
    let original_evidence = fs::read_to_string(&evidence_path).unwrap();
    let mut missing_artifact: Value = serde_json::from_str(&original_evidence).unwrap();
    missing_artifact
        .as_object_mut()
        .unwrap()
        .remove("artifact_hash");
    fs::write(
        &evidence_path,
        serde_json::to_string_pretty(&missing_artifact).unwrap() + "\n",
    )
    .unwrap();
    let rejected_missing_artifact = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected_missing_artifact.status.success());
    assert!(
        String::from_utf8_lossy(&rejected_missing_artifact.stdout)
            .contains("evidence_schema_invalid"),
        "missing artifact_hash must block deployed_blackbox evidence: {}",
        String::from_utf8_lossy(&rejected_missing_artifact.stdout)
    );
    fs::write(&evidence_path, &original_evidence).unwrap();

    let mut traversal_result: Value =
        serde_json::from_str(&fs::read_to_string(&result_path).unwrap()).unwrap();
    traversal_result["evidence_id"] = Value::String("../../outside".into());
    fs::write(
        &result_path,
        serde_json::to_string_pretty(&traversal_result).unwrap() + "\n",
    )
    .unwrap();
    let outside_evidence = root.join(".appsdk/records/outside.json");
    let mut outside_evidence_value: Value = serde_json::from_str(&original_evidence).unwrap();
    outside_evidence_value["evidence_id"] = Value::String("../../outside".into());
    fs::write(
        &outside_evidence,
        serde_json::to_string_pretty(&outside_evidence_value).unwrap() + "\n",
    )
    .unwrap();
    let rejected_traversal = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected_traversal.status.success());
    assert!(
        String::from_utf8_lossy(&rejected_traversal.stdout).contains("result_schema_invalid"),
        "path traversal evidence_id must block before any evidence read: {}",
        String::from_utf8_lossy(&rejected_traversal.stdout)
    );
    write_result("passed", "auth-1", "1", &head, "scenario-1", "passed");

    let mut missing_producer: Value =
        serde_json::from_str(&fs::read_to_string(&result_path).unwrap()).unwrap();
    missing_producer.as_object_mut().unwrap().remove("producer");
    fs::write(
        &result_path,
        serde_json::to_string_pretty(&missing_producer).unwrap() + "\n",
    )
    .unwrap();
    let rejected_missing_producer = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected_missing_producer.status.success());
    assert!(
        String::from_utf8_lossy(&rejected_missing_producer.stdout)
            .contains("result_schema_invalid"),
        "result without required producer fields must block: {}",
        String::from_utf8_lossy(&rejected_missing_producer.stdout)
    );
    write_result("passed", "auth-1", "1", &head, "scenario-1", "passed");

    let mut mismatched_producer: Value = serde_json::from_str(&original_evidence).unwrap();
    mismatched_producer["producer"]["identity"] = Value::String("other-worker".into());
    fs::write(
        &evidence_path,
        serde_json::to_string_pretty(&mismatched_producer).unwrap() + "\n",
    )
    .unwrap();
    let rejected_mismatched_producer = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected_mismatched_producer.status.success());
    assert!(
        String::from_utf8_lossy(&rejected_mismatched_producer.stdout).contains("evidence_mismatch"),
        "evidence producer mismatch must block: {}",
        String::from_utf8_lossy(&rejected_mismatched_producer.stdout)
    );
    fs::write(&evidence_path, &original_evidence).unwrap();

    let mut invalid_artifact_hash: Value = serde_json::from_str(&original_evidence).unwrap();
    invalid_artifact_hash["artifact_hash"] = Value::String("sha256:not-a-digest".into());
    fs::write(
        &evidence_path,
        serde_json::to_string_pretty(&invalid_artifact_hash).unwrap() + "\n",
    )
    .unwrap();
    let rejected_invalid_artifact_hash = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected_invalid_artifact_hash.status.success());
    assert!(
        String::from_utf8_lossy(&rejected_invalid_artifact_hash.stdout)
            .contains("evidence_schema_invalid"),
        "invalid artifact_hash must block deployed_blackbox evidence: {}",
        String::from_utf8_lossy(&rejected_invalid_artifact_hash.stdout)
    );
    fs::write(&evidence_path, &original_evidence).unwrap();

    let mut second_auth_manifest = manifest.clone();
    second_auth_manifest["effect_authorizations"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "authorization_id": "auth-2",
            "object_id": "app-core",
            "scenario_id": "scenario-1",
            "environment": "test",
            "allowed_effects": ["order created"],
            "valid_from": "2026-01-01T00:00:00Z",
            "valid_until": "2099-01-01T00:00:00Z",
            "approval_ref": "approval://owners/app-core"
        }));
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&second_auth_manifest).unwrap() + "\n",
    )
    .unwrap();
    write_result("passed", "auth-2", "1", &head, "scenario-1", "passed");
    let rejected_wrong_auth = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected_wrong_auth.status.success());
    assert!(
        String::from_utf8_lossy(&rejected_wrong_auth.stdout)
            .contains("effect_authorization_mismatch"),
        "result effect_authorization_id must equal the declared scenario authorization: {}",
        String::from_utf8_lossy(&rejected_wrong_auth.stdout)
    );
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap() + "\n",
    )
    .unwrap();
    write_result("passed", "auth-1", "1", &head, "scenario-1", "passed");

    let future_authorization = {
        let mut authorization = manifest.clone();
        authorization["effect_authorizations"][0]["valid_from"] =
            Value::String("2099-01-01T00:00:00Z".into());
        authorization["effect_authorizations"][0]["valid_until"] =
            Value::String("2099-12-31T00:00:00Z".into());
        authorization
    };
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&future_authorization).unwrap() + "\n",
    )
    .unwrap();
    let mut future_auth_result: Value =
        serde_json::from_str(&fs::read_to_string(&result_path).unwrap()).unwrap();
    future_auth_result["started_at"] = Value::String("2099-01-02T00:00:00Z".into());
    future_auth_result["finished_at"] = Value::String("2099-01-02T00:00:05Z".into());
    fs::write(
        &result_path,
        serde_json::to_string_pretty(&future_auth_result).unwrap() + "\n",
    )
    .unwrap();
    let future_auth = run(&["verify", "--test-admission", root_text]);
    assert!(!future_auth.status.success());
    assert!(
        String::from_utf8_lossy(&future_auth.stdout).contains("effect_authorization_expired"),
        "future valid_from with future result timestamps must block: {}",
        String::from_utf8_lossy(&future_auth.stdout)
    );
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap() + "\n",
    )
    .unwrap();
    write_result("passed", "auth-1", "1", &head, "scenario-1", "passed");

    let mut future_result: Value =
        serde_json::from_str(&fs::read_to_string(&result_path).unwrap()).unwrap();
    future_result["started_at"] = Value::String("2099-01-02T00:00:00Z".into());
    future_result["finished_at"] = Value::String("2099-01-02T00:00:05Z".into());
    fs::write(
        &result_path,
        serde_json::to_string_pretty(&future_result).unwrap() + "\n",
    )
    .unwrap();
    let future_result_run = run(&["verify", "--test-admission", root_text]);
    assert!(!future_result_run.status.success());
    assert!(
        String::from_utf8_lossy(&future_result_run.stdout).contains("effect_authorization_expired"),
        "future result timestamps must block before authorization can pass: {}",
        String::from_utf8_lossy(&future_result_run.stdout)
    );
    write_result("passed", "auth-1", "1", &head, "scenario-1", "passed");

    let mut inverted_result: Value =
        serde_json::from_str(&fs::read_to_string(&result_path).unwrap()).unwrap();
    inverted_result["started_at"] = Value::String("2026-01-02T00:00:10Z".into());
    inverted_result["finished_at"] = Value::String("2026-01-02T00:00:05Z".into());
    fs::write(
        &result_path,
        serde_json::to_string_pretty(&inverted_result).unwrap() + "\n",
    )
    .unwrap();
    let inverted_run = run(&["verify", "--test-admission", root_text]);
    assert!(!inverted_run.status.success());
    assert!(
        String::from_utf8_lossy(&inverted_run.stdout).contains("effect_authorization_expired"),
        "started_at after finished_at must block: {}",
        String::from_utf8_lossy(&inverted_run.stdout)
    );
    write_result("passed", "auth-1", "1", &head, "scenario-1", "passed");

    let legal_window = run(&["verify", "--test-admission", root_text]);
    assert!(
        legal_window.status.success(),
        "valid authorization window must pass; stdout={} stderr={}",
        String::from_utf8_lossy(&legal_window.stdout),
        String::from_utf8_lossy(&legal_window.stderr)
    );
    let legal_report: Value = serde_json::from_slice(&legal_window.stdout).unwrap();
    assert_eq!(legal_report["objects"][0]["status"], "passed");

    let mut command_cleanup_result: Value =
        serde_json::from_str(&fs::read_to_string(&result_path).unwrap()).unwrap();
    command_cleanup_result["cleanup_result"]["detail"] =
        Value::String("remove fixture; curl http://evil".into());
    fs::write(
        &result_path,
        serde_json::to_string_pretty(&command_cleanup_result).unwrap() + "\n",
    )
    .unwrap();
    let rejected_command_cleanup = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected_command_cleanup.status.success());
    assert!(
        String::from_utf8_lossy(&rejected_command_cleanup.stdout).contains("cleanup_failed"),
        "command-style cleanup detail must block optional test governance: {}",
        String::from_utf8_lossy(&rejected_command_cleanup.stdout)
    );
    write_result("passed", "auth-1", "1", &head, "scenario-1", "passed");

    let effect_missing_auth = {
        let mut effect = manifest.clone();
        effect["objects"][0]["scenarios"][0]
            .as_object_mut()
            .unwrap()
            .remove("effect_authorization_id");
        effect
    };
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&effect_missing_auth).unwrap() + "\n",
    )
    .unwrap();
    let rejected = run(&["verify", "--test-admission", root_text]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("TEST_GOVERNANCE_EFFECT_AUTHORIZATION_REQUIRED"),
        "stderr={}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}
