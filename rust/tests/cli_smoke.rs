use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_appsdk"))
}

fn temp_root(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("appsdk-rust-{name}-{}-{nonce}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    root
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
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(binary()).args(args).output().unwrap()
}

fn confirm_preparation(root: &PathBuf, project_root: &str, change_kind: &str) {
    fs::write(
        root.join(".appsdk-prepare.json"),
        format!(
            r#"{{"schema_version":1,"preparation_id":"prepare-test","status":"confirmed","objective":"test objective","change_kind":"{}","project_root":"{}","legacy_roots":["legacy"],"new_roots":["{}"],"protected_roots":["{}/protected"],"runtime_forbidden_roots":["{}/generated"],"boundary":{{"allowed_paths":["{}"],"forbidden_paths":["v3/**"],"payload_control_separation":"confirmed"}},"acceptance_criteria":["pass"],"non_goals":["v3"],"assumptions":[],"questions":[],"confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}}"#,
            change_kind, project_root, project_root, project_root, project_root, project_root
        ),
    )
    .unwrap();
}

#[test]
fn prepare_creates_template_and_init_rejects_unconfirmed_record() {
    let root = temp_root("prepare-gate");
    fs::create_dir_all(&root).unwrap();
    let root_text = root.to_str().unwrap();
    assert!(run(&["prepare", root_text]).status.success());
    let template = fs::read_to_string(root.join(".appsdk-prepare.json")).unwrap();
    assert!(template.contains("\"status\": \"draft\""));
    let init = run(&["init", root_text]);
    assert!(!init.status.success());
    assert!(String::from_utf8_lossy(&init.stderr).contains("PREPARATION_NOT_CONFIRMED"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_existing_project_creates_layout_and_manages_gitignore_idempotently() {
    let root = temp_root("init-existing");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(".gitignore"), "# project rules\nnode_modules/\n").unwrap();
    let root_text = root.to_str().unwrap();
    confirm_preparation(&root, ".", "project_refactor");

    let first = run(&["init", root_text]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(gitignore.starts_with("# project rules\nnode_modules/\n"));
    assert_eq!(gitignore.matches("# BEGIN APPSDK MANAGED").count(), 1);
    assert!(gitignore.contains(".appsdk-control/"));
    assert!(gitignore.contains(".appsdk/sdk.bin"));
    assert!(gitignore.contains("/active/lib/"));
    assert!(gitignore.contains("/generated/"));
    for path in [
        ".appsdk/project.json",
        ".appsdk/goal.json",
        ".appsdk/sdk.lock",
        ".appsdk/maps/resource-map.json",
        "playground/experiments",
        "active/lib",
        "protected/source",
        "protected/contracts",
        "protected/history",
        "generated",
        ".appsdk-control",
    ] {
        assert!(root.join(path).exists(), "missing {}", path);
    }

    let second = run(&["init", root_text]);
    assert!(second.status.success());
    let gitignore_after = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert_eq!(gitignore_after.matches("# BEGIN APPSDK MANAGED").count(), 1);
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_can_place_new_project_in_configured_subdirectory() {
    let workspace = temp_root("init-subdirectory");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(workspace.join("legacy.rs"), "legacy source\n").unwrap();
    let workspace_text = workspace.to_str().unwrap();
    confirm_preparation(&workspace, "next-code", "project_refactor");

    let result = run(&["init", workspace_text, "--project-root", "next-code"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let project = workspace.join("next-code");
    assert!(workspace.join("legacy.rs").exists());
    assert!(project.join(".appsdk/project.json").exists());
    assert!(project.join("playground/experiments").exists());
    assert!(project.join("active/lib").exists());
    assert!(project.join("protected/source").exists());
    assert!(project.join("generated").exists());
    assert!(fs::read_to_string(project.join(".gitignore"))
        .unwrap()
        .contains("/generated/"));
    assert!(run(&["verify", project.to_str().unwrap()]).status.success());

    let invalid = run(&["init", workspace_text, "--project-root", "../escape"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("INVALID_PROJECT_ROOT"));
    fs::remove_dir_all(workspace).unwrap();
}

fn pin_test_lock(root: &str) {
    let sdk_binary = binary();
    assert!(
        run(&["pin-lock", root, "--binary", sdk_binary.to_str().unwrap()])
            .status
            .success()
    );
}

fn canonical(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap(),
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|key| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap(),
                        canonical(&values[*key])
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn write_records(root: &PathBuf, module_id: &str, artifact_hash: &str, include_freeze: bool) {
    let records = root.join(".appsdk/records");
    fs::create_dir_all(&records).unwrap();
    fs::write(
        records.join(format!("evidence-record-{module_id}.json")),
        format!(
            r#"{{"evidence_id":"evidence-1","issue_id":"issue-1","experiment_id":"experiment-1","kind":"build","source_commit":"commit-1","artifact_hash":"{}","scope":{{"module_id":"{}"}},"producer":{{"adapter":"test","identity":"test"}},"result":"pass","created_at":"2026-01-01T00:00:00Z","expires_at":"2099-01-01T00:00:00Z","input_hashes":["input-1"],"scope_hash":"scope-1"}}"#,
            artifact_hash, module_id
        ),
    )
    .unwrap();
    fs::write(
        records.join(format!("review-record-{module_id}.json")),
        format!(
            r#"{{"review_id":"review-1","issue_id":"issue-1","promotion_id":"promotion-1","reviewer":{{"adapter":"test","identity":"test"}},"verdict":"pass","evidence_ids":["evidence-1"],"reviewed_commit":"commit-1","reviewed_artifact_hash":"{}","reviewed_scope_hash":"scope-1","ai_confidence":1.0,"confidence_rationale":"blackbox evidence","created_at":"2026-01-01T00:00:00Z"}}"#,
            artifact_hash
        ),
    )
    .unwrap();
    let promotion = format!(
        r#"{{"promotion_id":"promotion-1","issue_id":"issue-1","experiment_id":"experiment-1","module_id":"{}","base_commit":"base-1","source_commit":"commit-1","previous_active_version":null,"new_active_version":"active-v1","artifact_hash":"{}","scope_hash":"scope-1","public_api_hash":"api-1","review_id":"review-1","evidence_ids":["evidence-1"],"required_gate_results":[{{"gate_id":"blackbox","result":"pass","producer":"test"}}],"change_set_id":"change-1","compatibility_level":"compatible","root_cause":"test root cause","design_id":"design-1","change_reason_comment":"test reason","playground_cleanup_record_id":"cleanup-1","created_at":"2026-01-01T00:00:00Z"}}"#,
        module_id, artifact_hash
    );
    fs::write(
        records.join(format!("promotion-record-{module_id}.json")),
        &promotion,
    )
    .unwrap();
    fs::write(
        records.join(format!("playground-cleanup-cleanup-1.json")),
        r#"{"cleanup_id":"cleanup-1","disposition":"archive_then_remove","removed_paths":["playground/experiments/app-core"],"created_at":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    if include_freeze {
        let promotion_value: Value = serde_json::from_str(&promotion).unwrap();
        let promotion_hash = digest(&canonical(&promotion_value));
        fs::write(
            records.join(format!("freeze-record-{module_id}.json")),
            format!(
                r#"{{"freeze_id":"freeze-1","issue_id":"issue-1","module_id":"{}","promotion_id":"promotion-1","promotion_record_hash":"{}","artifact_record_id":"evidence-1","source_commit_or_tag":"commit-1","active_version":"active-v1","previous_active_version":null,"library_hash":"{}","public_api_hash":"api-1","review_id":"review-1","previous_active_immutable":false,"git_clean":true,"clean_scope":{{"base_commit":"base-1","changed_paths":[],"ignored_paths":[],"generated_policy":"tracked_hash"}},"owners":{{"vcs":"test","compiler":"test","api_extractor":"test","review":"test","artifact_registry":"test"}},"created_at":"2026-01-01T00:00:00Z"}}"#,
                module_id, promotion_hash, artifact_hash
            ),
        )
        .unwrap();
    }
}

fn write_regression_report(root: &PathBuf, module_id: &str, artifact_hash: &str) -> String {
    let report = serde_json::json!({
        "regression_report_id": "regression-app-core-v1",
        "module_id": module_id,
        "source_commit": "commit-1",
        "artifact_hash": artifact_hash,
        "public_api_hash": "api-1",
        "scope_hash": "scope-1",
        "input_hash": artifact_hash,
        "suite_id": "app-core-regression",
        "command": {
            "program": "cargo",
            "args": ["test", "--test", "app-core"],
            "working_directory": "."
        },
        "test_count": 1,
        "passed": 1,
        "failed": 0,
        "skipped": 0,
        "result": "pass",
        "producer": {
            "adapter": "cargo",
            "identity": "appsdk-regression-gate"
        },
        "created_at": "2026-01-01T00:00:00Z",
        "test_characteristics": {
            "whitebox": true,
            "blackbox": true
        }
    });
    let hash = digest(&canonical(&report));
    fs::write(
        root.join(format!(
            ".appsdk/records/regression-report-{module_id}.json"
        )),
        serde_json::to_string_pretty(&report).unwrap() + "\n",
    )
    .unwrap();
    hash
}

fn enable_regression_contract(root: &PathBuf) {
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["modules"][0]["regression"] = serde_json::json!({
        "required_before_freeze": true,
        "suite_id": "app-core-regression",
        "command": {
            "program": "cargo",
            "args": ["test", "--test", "app-core"],
            "working_directory": "."
        },
        "input_paths": ["playground/experiments/**"],
        "minimum_test_count": 1,
        "allow_skipped": false,
        "ordinary_mode_after_freeze": "disabled",
        "reenable_on": [
            "source_change",
            "contract_change",
            "public_api_change",
            "artifact_change",
            "dependency_change"
        ]
    });
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
}

#[test]
fn new_project_rejects_unconfirmed_compile_and_promote() {
    let root = temp_root("negative");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    init_git(&root);
    let compile = run(&["compile", root_text]);
    assert!(!compile.status.success());
    assert!(String::from_utf8_lossy(&compile.stderr).contains("GOAL_NOT_CONFIRMED:received"));
    let promote = run(&["promote", root_text, "--to", "source_implemented"]);
    assert!(!promote.status.success());
    assert!(String::from_utf8_lossy(&promote.stderr).contains("GOAL_NOT_CONFIRMED:received"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_allows_pending_clarification_but_compile_rejects_it() {
    let root = temp_root("clarification-pending");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let goal_file = root.join(".appsdk/goal.json");
    fs::write(&goal_file, r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"clarify","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":["scope"],"questions":[{"question_id":"q-1","question":"Which module?","status":"open"}],"status":"clarification_pending","confirmed_by":null,"confirmed_at":null,"created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    let verified = run(&["verify", root_text]);
    assert!(verified.status.success());
    let compile = run(&["compile", root_text]);
    assert!(!compile.status.success());
    assert!(!String::from_utf8_lossy(&compile.stderr).is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn confirmed_goal_and_pinned_lock_allow_compile_and_adjacent_promote() {
    let root = temp_root("positive");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let goal_file = root.join(".appsdk/goal.json");
    fs::write(&goal_file, r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    fs::write(root.join(".appsdk/sdk.lock"), format!(r#"{{"sdk":"appsdk","version":"0.1.0","digest":"sha256:{}","compiler_digest":"sha256:{}","contract_schema":1}}
"#, "a".repeat(64), "b".repeat(64))).unwrap();
    fs::write(root.join(".appsdk/project.json"), r#"{
  "schema_version": 1,
  "project_id": "change-me",
  "sdk": {"name": "appsdk", "version": "0.1.0"},
  "lifecycle": {"stage": "draft"},
  "access": {"protected_paths":[".appsdk/**"]},
  "governance": {"playground_root":"playground/**","active_root":"active/**","protected_root":"protected/**","generated_root":"generated/**","active_kind":"immutable_consumable_library","protected_kinds":["source"],"generated_kinds":["compiler_output"],"freeze_requirements":["git_clean"],"promotion_requires":["evidence"],"runtime_forbidden_roots":["playground/**"],"record_contracts":["contracts/records/evidence-record.schema.json","contracts/records/goal-clarification-record.schema.json","contracts/records/review-record.schema.json","contracts/records/promotion-record.schema.json","contracts/records/regression-report.schema.json","contracts/records/freeze-record.schema.json","contracts/records/record-graph.contract.json"],"zone_transition_contract":"contracts/transitions/zone-transition-manifest.json","playground_retention":"archive_then_remove","debug_merge_comment_required":true},
  "lifecycles": {"issue":"open","library":"draft","source_snapshot":"mutable","artifact":"generated"},
    "modules": [{"module_id":"app-core","stage":"source_implemented","owned_paths":["playground/experiments/**"],"source_owner":"app-core","active_artifact":"active/lib/app-core/**","generated_outputs":["generated/**"],"contract_paths":["contracts/records/**","contracts/transitions/**"],"dependency_modules":[],"build":{"program":"sh","args":["-c","mkdir -p generated/modules/app-core/lib && printf 'app-core placeholder\\n' > generated/modules/app-core/lib/app-core.placeholder"],"working_directory":"."},"artifact_paths":["app-core.placeholder"],"regression":{"required_before_freeze":true,"suite_id":"app-core-regression","command":{"program":"cargo","args":["test"],"working_directory":"."},"input_paths":["playground/experiments/**"],"minimum_test_count":1,"allow_skipped":false,"ordinary_mode_after_freeze":"disabled","reenable_on":["source_change","contract_change","public_api_change","artifact_change","dependency_change"]}}]
}
"#).unwrap();
    pin_test_lock(root_text);
    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    let compile = run(&["compile", root_text]);
    assert!(compile.status.success());
    assert!(root.join("generated/project.compiled.json").exists());
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "contract_bound",
    ])
    .status
    .success());
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "compiled",
    ])
    .status
    .success());
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "controlled_verified",
    ])
    .status
    .success());
    let module = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(!module.status.success());
    assert!(String::from_utf8_lossy(&module.stderr)
        .contains("MISSING_RECORD:evidence-record-app-core.json"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn freeze_rejects_without_architecture_stage_and_records() {
    let root = temp_root("freeze");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let result = run(&["freeze", root_text, "--module", "app-core"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("GOAL_NOT_CONFIRMED:received"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn active_publish_rejects_unfrozen_module() {
    let root = temp_root("active");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    let result = run(&[
        "publish-active",
        root_text,
        "--module",
        "app-core",
        "--version",
        "v1",
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr)
        .contains("ACTIVE_PUBLISH_REQUIRES_FROZEN_MODULE:app-core"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn full_module_freeze_and_active_publish_require_record_graph() {
    let root = temp_root("full-lifecycle");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    enable_regression_contract(&root);
    init_git(&root);
    fs::write(root.join(".appsdk/goal.json"), r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    fs::write(root.join(".appsdk/sdk.lock"), format!(r#"{{"sdk":"appsdk","version":"0.1.0","digest":"sha256:{}","compiler_digest":"sha256:{}","contract_schema":1}}
"#, "a".repeat(64), "b".repeat(64))).unwrap();
    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    project["lifecycle"]["stage"] = Value::String("draft".into());
    project["modules"][0]["stage"] = Value::String("source_implemented".into());
    project["modules"][0]["owned_paths"] = serde_json::json!(["playground/experiments/**"]);
    project["modules"][0]["generated_outputs"] = serde_json::json!(["generated/**"]);
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    pin_test_lock(root_text);
    let source_promote = run(&["promote", root_text, "--to", "source_implemented"]);
    assert!(source_promote.status.success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    let edge_compile = run(&["compile", root_text]);
    assert!(
        edge_compile.status.success(),
        "{}",
        String::from_utf8_lossy(&edge_compile.stderr)
    );
    assert!(run(&["promote", root_text, "--to", "compiled"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "controlled_verified"])
        .status
        .success());
    let edge_compile = run(&["compile", root_text]);
    assert!(
        edge_compile.status.success(),
        "{}",
        String::from_utf8_lossy(&edge_compile.stderr)
    );
    for stage in ["contract_bound", "compiled", "controlled_verified"] {
        assert!(run(&[
            "promote-module",
            root_text,
            "--module",
            "app-core",
            "--to",
            stage
        ])
        .status
        .success());
    }
    assert!(!run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable"
    ])
    .status
    .success());
    let module_artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    let architecture_hash = module_artifact
        .get("artifact_hash")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    write_records(&root, "app-core", &architecture_hash, false);
    let architecture = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable",
    ]);
    assert!(
        architecture.status.success(),
        "{}",
        String::from_utf8_lossy(&architecture.stderr)
    );
    assert!(!run(&["freeze", root_text, "--module", "app-core"])
        .status
        .success());
    write_records(&root, "app-core", &architecture_hash, true);
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "promotion-records"])
        .status()
        .unwrap()
        .success());
    let missing_regression = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "frozen",
    ]);
    assert!(!missing_regression.status.success());
    assert!(String::from_utf8_lossy(&missing_regression.stderr)
        .contains("MISSING_RECORD:regression-report-app-core.json"));
    let regression_report_hash = write_regression_report(&root, "app-core", &architecture_hash);
    let freeze_record = root.join(".appsdk/records/freeze-record-app-core.json");
    let mut freeze_record_value: Value =
        serde_json::from_str(&fs::read_to_string(&freeze_record).unwrap()).unwrap();
    freeze_record_value["regression_report_id"] = Value::String("regression-app-core-v1".into());
    freeze_record_value["regression_report_hash"] = Value::String(regression_report_hash);
    fs::write(
        &freeze_record,
        serde_json::to_string_pretty(&freeze_record_value).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "regression-report"])
        .status()
        .unwrap()
        .success());
    let freeze = run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "frozen",
    ]);
    assert!(
        freeze.status.success(),
        "{}",
        String::from_utf8_lossy(&freeze.stderr)
    );
    assert!(root
        .join("protected/history/app-core/freeze-artifact.json")
        .exists());
    assert!(root
        .join("protected/history/app-core/module-contract.json")
        .exists());
    assert!(root
        .join("protected/history/app-core/source-snapshot.json")
        .exists());
    let pub_result = run(&[
        "publish-active",
        root_text,
        "--module",
        "app-core",
        "--version",
        "active-v1",
    ]);
    assert!(pub_result.status.success());
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    assert!(root
        .join("active/lib/app-core/active-v1/artifact.json")
        .exists());
    let duplicate = run(&[
        "publish-active",
        root_text,
        "--module",
        "app-core",
        "--version",
        "active-v1",
    ]);
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("ACTIVE_VERSION_EXISTS"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn frozen_module_keeps_other_modules_mutable() {
    let root = temp_root("module-independence");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    enable_regression_contract(&root);
    init_git(&root);
    fs::write(root.join(".appsdk/goal.json"), r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    fs::write(root.join(".appsdk/sdk.lock"), format!(r#"{{"sdk":"appsdk","version":"0.1.0","digest":"sha256:{}","compiler_digest":"sha256:{}","contract_schema":1}}
"#, "a".repeat(64), "b".repeat(64))).unwrap();
    pin_test_lock(root_text);

    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    assert!(run(&["compile", root_text]).status.success());
    assert!(run(&["promote", root_text, "--to", "compiled"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "controlled_verified"])
        .status
        .success());
    for stage in ["contract_bound", "compiled", "controlled_verified"] {
        assert!(run(&[
            "promote-module",
            root_text,
            "--module",
            "app-core",
            "--to",
            stage
        ])
        .status
        .success());
    }
    let module_artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    let architecture_hash = module_artifact
        .get("artifact_hash")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    write_records(&root, "app-core", &architecture_hash, false);
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable"
    ])
    .status
    .success());
    write_records(&root, "app-core", &architecture_hash, true);
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "promotion-records"])
        .status()
        .unwrap()
        .success());
    let regression_report_hash = write_regression_report(&root, "app-core", &architecture_hash);
    let freeze_record = root.join(".appsdk/records/freeze-record-app-core.json");
    let mut freeze_record_value: Value =
        serde_json::from_str(&fs::read_to_string(&freeze_record).unwrap()).unwrap();
    freeze_record_value["regression_report_id"] = Value::String("regression-app-core-v1".into());
    freeze_record_value["regression_report_hash"] = Value::String(regression_report_hash);
    fs::write(
        &freeze_record,
        serde_json::to_string_pretty(&freeze_record_value).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "regression-report"])
        .status()
        .unwrap()
        .success());
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "frozen"
    ])
    .status
    .success());
    assert!(run(&[
        "publish-active",
        root_text,
        "--module",
        "app-core",
        "--version",
        "active-v1"
    ])
    .status
    .success());
    // Protected archive must contain the frozen module's source, library,
    // contract files, module contract, and hashes, so a frozen module is a
    // self-contained audit unit.
    for path in [
        "protected/history/app-core/source/playground/experiments",
        "protected/history/app-core/library/app-core.placeholder",
        "protected/history/app-core/contracts/records/evidence-record.schema.json",
        "protected/history/app-core/contracts/transitions/zone-transition-manifest.json",
        "protected/history/app-core/module-contract.json",
        "protected/history/app-core/freeze-artifact.json",
    ] {
        assert!(
            root.join(path).exists(),
            "protected archive is incomplete: {path}"
        );
    }
    let frozen_artifact_hash = {
        let value: Value = serde_json::from_str(
            &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json"))
                .unwrap(),
        )
        .unwrap();
        value
            .get("artifact_hash")
            .and_then(Value::as_str)
            .unwrap()
            .to_string()
    };

    let project_file = root.join(".appsdk/project.json");
    let mut project: Value =
        serde_json::from_str(&fs::read_to_string(&project_file).unwrap()).unwrap();
    let app_core_regression = project["modules"][0]["regression"].clone();
    project["modules"].as_array_mut().unwrap().push(serde_json::json!({
        "module_id": "app-edge",
        "stage": "source_implemented",
        "owned_paths": ["playground/experiments-edge/**"],
        "source_owner": "app-edge",
        "active_artifact": "active/lib/app-edge/**",
        "generated_outputs": ["generated/**"],
        "contract_paths": ["contracts/records/**", "contracts/transitions/**"],
        "dependency_modules": [],
        "build": {
            "program": "sh",
            "args": ["-c", "mkdir -p generated/modules/app-edge/lib && printf 'app-edge placeholder\\n' > generated/modules/app-edge/lib/app-edge.placeholder"],
            "working_directory": "."
        },
        "artifact_paths": ["app-edge.placeholder"],
        "regression": app_core_regression
    }));
    fs::create_dir_all(root.join("playground/experiments-edge")).unwrap();
    fs::write(
        &project_file,
        serde_json::to_string_pretty(&project).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "add app-edge module"])
        .status()
        .unwrap()
        .success());

    assert!(run(&["compile", root_text]).status.success());
    let frozen_after: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        frozen_after
            .get("artifact_hash")
            .and_then(Value::as_str)
            .unwrap(),
        frozen_artifact_hash
    );
    let edge_artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-edge/module.compiled.json")).unwrap(),
    )
    .unwrap();
    assert_ne!(
        edge_artifact
            .get("artifact_hash")
            .and_then(Value::as_str)
            .unwrap(),
        frozen_artifact_hash
    );
    for stage in ["contract_bound", "compiled", "controlled_verified"] {
        let promoted = run(&[
            "promote-module",
            root_text,
            "--module",
            "app-edge",
            "--to",
            stage,
        ]);
        assert!(
            promoted.status.success(),
            "promote app-edge {} failed: {}",
            stage,
            String::from_utf8_lossy(&promoted.stderr)
        );
    }
    let edge_hash = edge_artifact
        .get("artifact_hash")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    write_records(&root, "app-edge", &edge_hash, false);
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "app-edge records"])
        .status()
        .unwrap()
        .success());
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-edge",
        "--to",
        "architecture_stable"
    ])
    .status
    .success());
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn begin_version_preserves_v1_and_opens_a_version_bound_source_stage() {
    let root = temp_root("begin-version");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    enable_regression_contract(&root);
    init_git(&root);
    fs::write(root.join(".appsdk/goal.json"), r#"{"goal_id":"goal-1","raw_request":"change","understood_objective":"change","acceptance_criteria":["pass"],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"confirmed","confirmed_by":"test","confirmed_at":"2026-01-01T00:00:00Z","created_at":"2026-01-01T00:00:00Z"}
"#).unwrap();
    pin_test_lock(root_text);
    assert!(run(&["promote", root_text, "--to", "source_implemented"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "contract_bound"])
        .status
        .success());
    assert!(run(&["compile", root_text]).status.success());
    assert!(run(&["promote", root_text, "--to", "compiled"])
        .status
        .success());
    assert!(run(&["promote", root_text, "--to", "controlled_verified"])
        .status
        .success());
    for stage in ["contract_bound", "compiled", "controlled_verified"] {
        assert!(run(&[
            "promote-module",
            root_text,
            "--module",
            "app-core",
            "--to",
            stage
        ])
        .status
        .success());
    }
    let module_artifact: Value = serde_json::from_str(
        &fs::read_to_string(root.join("generated/modules/app-core/module.compiled.json")).unwrap(),
    )
    .unwrap();
    let v1_hash = module_artifact["artifact_hash"]
        .as_str()
        .unwrap()
        .to_string();
    write_records(&root, "app-core", &v1_hash, false);
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "architecture_stable"
    ])
    .status
    .success());
    write_records(&root, "app-core", &v1_hash, true);
    let regression_hash = write_regression_report(&root, "app-core", &v1_hash);
    let freeze_file = root.join(".appsdk/records/freeze-record-app-core.json");
    let mut freeze: Value =
        serde_json::from_str(&fs::read_to_string(&freeze_file).unwrap()).unwrap();
    freeze["regression_report_id"] = Value::String("regression-app-core-v1".into());
    freeze["regression_report_hash"] = Value::String(regression_hash);
    fs::write(
        &freeze_file,
        serde_json::to_string_pretty(&freeze).unwrap() + "\n",
    )
    .unwrap();
    assert!(Command::new("git")
        .args(["-C", root_text, "add", "."])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["-C", root_text, "commit", "-m", "freeze-v1"])
        .status()
        .unwrap()
        .success());
    assert!(run(&[
        "promote-module",
        root_text,
        "--module",
        "app-core",
        "--to",
        "frozen"
    ])
    .status
    .success());
    assert!(run(&[
        "publish-active",
        root_text,
        "--module",
        "app-core",
        "--version",
        "active-v1"
    ])
    .status
    .success());
    let active_v1 = root.join("active/lib/app-core/active-v1/artifact.json");
    let active_v1_text = fs::read_to_string(&active_v1).unwrap();

    let wrong_from = run(&[
        "begin-version",
        root_text,
        "--module",
        "app-core",
        "--from",
        "active-v0",
        "--to",
        "active-v2",
    ]);
    assert!(!wrong_from.status.success());
    assert!(String::from_utf8_lossy(&wrong_from.stderr).contains("MODULE_VERSION_FROM_NOT_CURRENT"));

    let opened = run(&[
        "begin-version",
        root_text,
        "--module",
        "app-core",
        "--from",
        "active-v1",
        "--to",
        "active-v2",
    ]);
    assert!(
        opened.status.success(),
        "{}",
        String::from_utf8_lossy(&opened.stderr)
    );
    let project: Value =
        serde_json::from_str(&fs::read_to_string(root.join(".appsdk/project.json")).unwrap())
            .unwrap();
    assert_eq!(project["modules"][0]["stage"], "source_implemented");
    assert_eq!(
        project["modules"][0]["version_base"],
        serde_json::json!({
            "previous_active_version": "active-v1",
            "new_active_version": "active-v2",
            "base_artifact_hash": v1_hash,
            "base_source_commit": "commit-1"
        })
    );
    assert_eq!(fs::read_to_string(&active_v1).unwrap(), active_v1_text);
    assert!(root
        .join("protected/history-versions/app-core/active-v1/freeze-artifact.json")
        .is_file());
    assert!(!root.join("protected/history/app-core").exists());
    assert!(root
        .join(".appsdk/records/history/app-core/active-v1/freeze-record-app-core.json")
        .is_file());
    let verified = run(&["verify", root_text]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    fs::remove_dir_all(root).unwrap();
}
