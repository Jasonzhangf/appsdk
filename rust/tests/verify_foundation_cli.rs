//! Public `appsdk verify` coverage for the migrated four-node foundation.
//!
//! The foundation graph (`load_project`, `governance_baseline`,
//! `declared_contracts`, `project_contract`) is the only migrated segment. The
//! inputs below are checked-in static fixtures. Setup copies them into a temp
//! project and mirrors the install layout of the embedded SDK bundle by reading
//! `contracts/sdk-bundle.manifest.json`; it never runs `new`, `init`, `compile`,
//! `pin-lock`, or `freeze`, and never computes a checksum.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_appsdk"))
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("rust manifest has a repository parent")
        .to_path_buf()
}

fn fixture_root(name: &str) -> PathBuf {
    repo_root()
        .join("rust/tests/fixtures/verify_foundation")
        .join(name)
}

fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "appsdk-verify-foundation-cli-{label}-{}-{nonce}",
        std::process::id()
    ));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    root
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let destination = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn write_json(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

/// Materializes one project from the static fixtures. The draft fixture
/// supplies the shared governance maps; the named fixture overrides project and
/// lock. Installed resource placeholders are created first for the compiled
/// fixture, then canonical record/transition contracts and the
/// development-scenario manifest are copied from the repository so the declared
/// contracts match the embedded bundle without duplicating their text here.
fn setup(name: &str, label: &str) -> PathBuf {
    let root = temp_root(label);
    copy_dir(&fixture_root("draft"), &root);
    if name != "draft" {
        copy_dir(&fixture_root(name), &root);
    }
    if name == "compiled" {
        materialize_bundle_resources(&root);
        write_compiled_artifact(&root);
    }
    copy_dir(
        &repo_root().join("contracts/records"),
        &root.join("contracts/records"),
    );
    copy_dir(
        &repo_root().join("contracts/transitions"),
        &root.join("contracts/transitions"),
    );
    fs::create_dir_all(root.join(".appsdk/contracts")).unwrap();
    fs::copy(
        repo_root().join("contracts/development-scenarios.manifest.json"),
        root.join(".appsdk/contracts/development-scenarios.manifest.json"),
    )
    .unwrap();
    pin_release_version(&root);
    root
}

/// Keeps the fixture's SDK pin equal to the binary under test without a hash or
/// Git lookup: the release version is checked-in build input for `build.rs`.
fn pin_release_version(root: &Path) {
    let version = fs::read_to_string(repo_root().join("rust/release-version"))
        .unwrap()
        .trim()
        .to_string();
    let project_path = root.join(".appsdk/project.json");
    let mut project = read_json(&project_path);
    project["sdk"]["version"] = Value::String(version.clone());
    write_json(&project_path, &project);

    let lock_path = root.join(".appsdk/sdk.lock");
    let mut lock = read_json(&lock_path);
    lock["version"] = Value::String(version);
    write_json(&lock_path, &lock);
}

/// Mirrors the embedded bundle's install layout for a compiled fixture. The
/// resource list comes from the checked-in bundle manifest, which is the same
/// source the binary embeds, so the fixture tracks bundle changes instead of
/// freezing a stale record. Digest fields are shape-valid placeholders; the
/// migrated public path is the semantic (content-identity-free) resource check.
fn materialize_bundle_resources(root: &Path) {
    let version = fs::read_to_string(repo_root().join("rust/release-version"))
        .unwrap()
        .trim()
        .to_string();
    let manifest = read_json(&repo_root().join("contracts/sdk-bundle.manifest.json"));
    let mut resources = Vec::new();
    for (class, sources) in manifest["resources"].as_object().unwrap() {
        for source in sources.as_array().unwrap() {
            let source = source.as_str().unwrap();
            let path = install_relative(source, class);
            let target = root.join(&path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(
                &target,
                format!("verify-foundation fixture resource: {source}\n"),
            )
            .unwrap();
            resources.push(json!({
                "source": source,
                "class": class,
                "path": path,
                "digest": "sha256:2222222222222222222222222222222222222222222222222222222222222222",
            }));
        }
    }
    write_json(
        &root.join(".appsdk/sdk-resources.json"),
        &json!({
            "schema_version": 1,
            "sdk": "appsdk",
            "version": version,
            "bundle_digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "manifest_digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
            "resources": resources,
        }),
    );
}

/// Install-relative path for one bundle resource. This mirrors the SDK install
/// layout so the fixture presents the files the semantic resource check reads.
fn install_relative(source: &str, class: &str) -> String {
    match class {
        "contracts" => format!(
            ".appsdk/contracts/{}",
            source.trim_start_matches("contracts/")
        ),
        "docs" => format!(".appsdk/docs/{}", source.trim_start_matches("docs/")),
        "skills" => format!(".appsdk/skills/{}", source.trim_start_matches("skills/")),
        "rules" => ".appsdk/rules/appsdk-project-governance.md".to_string(),
        other => panic!("unknown bundle resource class: {other}"),
    }
}

/// Builds the compiled artifact from the project's own module surfaces so the
/// artifact schema/module semantics hold without a compile producer.
fn write_compiled_artifact(root: &Path) {
    let project = read_json(&root.join(".appsdk/project.json"));
    let modules = project["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|module| {
            json!({
                "module_id": module["module_id"],
                "stage": module["stage"],
                "source_owner": module["source_owner"],
                "active_artifact": module["active_artifact"],
                "owned_paths": module["owned_paths"],
                "generated_outputs": module["generated_outputs"],
                "regression": module["regression"],
            })
        })
        .collect::<Vec<_>>();
    write_json(
        &root.join("generated/project.compiled.json"),
        &json!({
            "artifact_schema": 1,
            "project_id": project["project_id"],
            "sdk": project["sdk"],
            "modules": modules,
        }),
    );
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .env("APPSDK_HOME", root.with_extension("appsdk-home"))
        .env_remove("GIT_BUG_BIN")
        .env_remove("TMUX_PANE")
        .output()
        .unwrap()
}

fn verify(root: &Path, admission: bool) -> Output {
    let root_text = root.to_str().unwrap();
    if admission {
        run(root, &["verify", "--admission", root_text])
    } else {
        run(root, &["verify", root_text])
    }
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "verify must succeed; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn invocation_records(root: &Path) -> Vec<Value> {
    let path = root.join(".appsdk-control/command-invocations.jsonl");
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn foundation_is_registered_as_an_executable_graph() {
    let root = temp_root("graph-registration");
    let result = stdout_json(&run(&root, &["dagpipe", "validate"]));
    assert_eq!(result["compiled_registry"], json!(true));
    let graph = result["graphs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|graph| graph["id"] == "appsdk-verify-foundation")
        .expect("foundation graph must be discoverable through the public registry");
    assert_eq!(graph["binding_kind"], json!("executable"));
}

/// Fail mode: the foundation graph is not consumed, so a legal draft project
/// loses its public result or reports the wrong lifecycle/off-governance fields.
#[test]
fn draft_default_and_admission_report_off_governance() {
    let root = setup("draft", "draft-default-admission");

    let default = stdout_json(&verify(&root, false));
    assert_eq!(default["command_ok"], json!(true));
    assert_eq!(default["ok"], json!(false));
    assert_eq!(default["delivery_assessed"], json!(false));
    assert_eq!(default["delivery_verified"], json!(false));
    assert_eq!(default["reason"], json!("delivery_not_evaluated"));
    assert_eq!(default["stage"], json!("draft"));
    assert_eq!(default["test_governance"]["mode"], json!("off"));
    assert_eq!(default["test_governance"]["status"], json!("not_selected"));

    let admission = stdout_json(&verify(&root, true));
    assert_eq!(admission["command_ok"], json!(true));
    assert_eq!(admission["ok"], json!(true));
    assert_eq!(admission["delivery_assessed"], json!(true));
    assert_eq!(admission["delivery_verified"], json!(true));
    assert_eq!(admission["reason"], Value::Null);
    assert_eq!(admission["stage"], json!("draft"));
    assert_eq!(
        admission["test_governance"]["status"],
        json!("not_selected")
    );

    fs::remove_dir_all(&root).ok();
}

#[test]
fn unknown_project_stage_is_rejected_in_default_and_admission() {
    for admission in [false, true] {
        let root = setup(
            "draft",
            if admission {
                "unknown-project-stage-admission"
            } else {
                "unknown-project-stage-default"
            },
        );
        let project_path = root.join(".appsdk/project.json");
        let mut project = read_json(&project_path);
        assert_eq!(project["modules"][0]["stage"], json!("draft"));
        project["lifecycle"]["stage"] = json!("ghost");
        write_json(&project_path, &project);

        let output = verify(&root, admission);
        assert!(!output.status.success(), "unknown project stage must fail");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).trim(),
            "UNKNOWN_PROJECT_STAGE:ghost"
        );
        assert!(output.stdout.is_empty());
        let records = invocation_records(&root);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["outcome"], json!("invoked"));
        assert_eq!(
            records[0]["selection"],
            json!(if admission { "admission" } else { "default" })
        );
        fs::remove_dir_all(&root).ok();
    }
}

/// Fail mode: the compiled public path stops consuming the foundation result or
/// enters the selected test-governance branch even though mode is off.
#[test]
fn compiled_off_governance_default_and_admission_pass() {
    let root = setup("compiled", "compiled-default-admission");

    let default = stdout_json(&verify(&root, false));
    assert_eq!(default["command_ok"], json!(true));
    assert_eq!(default["delivery_assessed"], json!(false));
    assert_eq!(default["reason"], json!("delivery_not_evaluated"));
    assert_eq!(default["stage"], json!("compiled"));
    assert_eq!(default["test_governance"]["status"], json!("not_selected"));

    let admission = stdout_json(&verify(&root, true));
    assert_eq!(admission["command_ok"], json!(true));
    assert_eq!(admission["ok"], json!(true));
    assert_eq!(admission["delivery_assessed"], json!(true));
    assert_eq!(
        admission["test_governance"]["status"],
        json!("not_selected")
    );

    fs::remove_dir_all(&root).ok();
}

/// Fail mode: the changed resource and compiled-artifact semantic guards stop
/// reaching the public boundary, fail later, or replace their first business
/// error while also changing the declared invocation-log semantics.
#[test]
fn semantic_guard_failures_report_the_first_error() {
    struct Case {
        label: &'static str,
        mutate: fn(&Path),
        expected: &'static str,
    }

    let cases = [
        Case {
            label: "missing-installed-memory-resource",
            mutate: |root| {
                fs::remove_file(root.join(".appsdk/contracts/memory/memory-entry.schema.json"))
                    .unwrap();
            },
            expected: "SDK_RESOURCE_MISMATCH:.appsdk/contracts/memory/memory-entry.schema.json",
        },
        Case {
            label: "artifact-project-id-mismatch",
            mutate: |root| {
                let path = root.join("generated/project.compiled.json");
                let mut artifact = read_json(&path);
                artifact["project_id"] = json!("other-project");
                write_json(&path, &artifact);
            },
            expected: "INVALID_ARTIFACT_SCHEMA",
        },
        Case {
            label: "artifact-module-owner-mismatch",
            mutate: |root| {
                let path = root.join("generated/project.compiled.json");
                let mut artifact = read_json(&path);
                artifact["modules"][0]["source_owner"] = json!("other-owner");
                write_json(&path, &artifact);
            },
            expected: "ARTIFACT_MODULE_MISMATCH:app-core",
        },
    ];

    for case in cases {
        for admission in [false, true] {
            let root = setup("compiled", case.label);
            let project = read_json(&root.join(".appsdk/project.json"));
            assert_eq!(project["test_governance"]["mode"], json!("off"));
            (case.mutate)(&root);

            let output = verify(&root, admission);
            assert!(
                !output.status.success(),
                "{} (admission={admission}) must fail",
                case.label
            );
            assert!(
                output.stdout.is_empty(),
                "{} (admission={admission}) must not emit a business result; stdout={}",
                case.label,
                String::from_utf8_lossy(&output.stdout)
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                stderr.trim(),
                case.expected,
                "{} (admission={admission}) first error: expected {expected}, stderr={stderr}",
                case.label,
                expected = case.expected
            );

            let records = invocation_records(&root);
            assert_eq!(
                records.len(),
                1,
                "{} (admission={admission}) must append one invocation record",
                case.label
            );
            assert_eq!(records[0]["command"], json!("verify"));
            assert_eq!(
                records[0]["selection"],
                json!(if admission { "admission" } else { "default" })
            );
            assert_eq!(records[0]["outcome"], json!("invoked"));
            assert!(
                records[0].get("error_code").is_none(),
                "{} (admission={admission}) log changed on failure: {:?}",
                case.label,
                records[0]
            );

            fs::remove_dir_all(&root).ok();
        }
    }
}

/// Fail mode: the first foundation error is reordered, swallowed, or replaced
/// by a later node's error, so the public message no longer identifies the
/// actual first failing node.
#[test]
fn foundation_error_reports_the_original_first_error() {
    struct Case {
        label: &'static str,
        mutate: fn(&Path),
        expected: &'static str,
    }

    let cases = [
        Case {
            label: "broken-project-json",
            mutate: |root| {
                fs::write(root.join(".appsdk/project.json"), "{ not json").unwrap();
            },
            expected: "INVALID_PROJECT_CONTRACT",
        },
        Case {
            label: "broken-governance-map",
            mutate: |root| {
                fs::write(root.join(".appsdk/maps/resource-map.json"), "{ not json").unwrap();
            },
            expected: "INVALID_GOVERNANCE_MAP:resource-map.json",
        },
        Case {
            label: "map-error-precedes-declared-error",
            mutate: |root| {
                fs::write(root.join(".appsdk/maps/resource-map.json"), "{ not json").unwrap();
                let path = root.join(".appsdk/project.json");
                let mut project = read_json(&path);
                project["governance"]["record_contracts"] = json!([]);
                write_json(&path, &project);
            },
            expected: "INVALID_GOVERNANCE_MAP:resource-map.json",
        },
        Case {
            label: "weakened-record-contract",
            mutate: |root| {
                let path = root.join("contracts/records/evidence-record.schema.json");
                let mut schema = read_json(&path);
                schema.as_object_mut().unwrap().remove("allOf");
                write_json(&path, &schema);
            },
            expected: "DECLARED_RECORD_CONTRACT_MISMATCH:contracts/records/evidence-record.schema.json/allOf",
        },
        Case {
            label: "overlapping-module-ownership",
            mutate: |root| {
                let path = root.join(".appsdk/project.json");
                let mut project = read_json(&path);
                let first = project["modules"][0].clone();
                let mut second = first.clone();
                second["module_id"] = json!("app-second");
                second["source_owner"] = json!("app-second");
                second["active_artifact"] = json!("active/lib/app-second/**");
                project["modules"] = json!([first, second]);
                write_json(&path, &project);
            },
            expected: "OVERLAPPING_MODULE_OWNERSHIP",
        },
    ];

    for case in cases {
        for admission in [false, true] {
            let root = setup("draft", case.label);
            (case.mutate)(&root);
            let output = verify(&root, admission);
            assert!(
                !output.status.success(),
                "{} (admission={admission}) must fail",
                case.label
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                stderr.trim(),
                case.expected,
                "{} (admission={admission}) first error: expected {expected}, stderr={stderr}",
                case.label,
                expected = case.expected
            );
            fs::remove_dir_all(&root).ok();
        }
    }
}

/// Fail mode: a selected project reaches the test-governance identity lookup
/// even though the foundation already failed, or the foundation error is
/// replaced by an identity error.
#[test]
fn selected_project_fails_in_foundation_before_identity_query() {
    let root = setup("draft", "selected-foundation");
    let project_path = root.join(".appsdk/project.json");
    let mut project = read_json(&project_path);
    project["test_governance"] = json!({
        "mode": "selected",
        "manifest": ".appsdk/test-governance.json"
    });
    write_json(&project_path, &project);
    fs::copy(
        fixture_root(".").join("selected-test-governance.json"),
        root.join(".appsdk/test-governance.json"),
    )
    .unwrap();
    fs::write(root.join(".appsdk/maps/resource-map.json"), "{ not json").unwrap();

    let output = verify(&root, false);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("INVALID_GOVERNANCE_MAP:resource-map.json"),
        "foundation error must be reported first, stderr={stderr}"
    );
    assert!(
        !stderr.contains("TEST_GOVERNANCE_CANDIDATE_COMMIT_UNAVAILABLE"),
        "selected project must stop before the identity query, stderr={stderr}"
    );

    fs::remove_dir_all(&root).ok();
}

/// Fail mode: verify changes declared project, lock, resource-record, or
/// compiled-artifact business state, writes into its external home, or fails to
/// append the single public invocation record with the requested selection.
#[test]
fn verify_logs_once_and_leaves_project_files_unchanged() {
    let root = setup("compiled", "log-side-effects");
    let project_path = root.join(".appsdk/project.json");
    let lock_path = root.join(".appsdk/sdk.lock");
    let resources_path = root.join(".appsdk/sdk-resources.json");
    let artifact_path = root.join("generated/project.compiled.json");
    let project_before = read_json(&project_path);
    let lock_before = read_json(&lock_path);
    let resources_before = read_json(&resources_path);
    let artifact_before = read_json(&artifact_path);
    assert_eq!(project_before["lifecycle"]["stage"], json!("compiled"));
    assert_eq!(project_before["modules"][0]["module_id"], json!("app-core"));
    assert_eq!(lock_before["sdk"], json!("appsdk"));
    assert_eq!(artifact_before["project_id"], project_before["project_id"]);

    let external_home = root.with_extension("appsdk-home");
    fs::create_dir_all(&external_home).unwrap();
    let sentinel = external_home.join("sentinel.txt");
    fs::write(&sentinel, "keep this content\n").unwrap();

    stdout_json(&verify(&root, false));
    stdout_json(&verify(&root, true));

    let records = invocation_records(&root);
    assert_eq!(records.len(), 2, "one invoked record per public verify");
    assert_eq!(records[0]["command"], json!("verify"));
    assert_eq!(records[0]["selection"], json!("default"));
    assert_eq!(records[0]["outcome"], json!("invoked"));
    assert!(records[0]["started_at"].as_str().unwrap_or("").len() > 10);
    assert_eq!(records[1]["selection"], json!("admission"));

    assert_eq!(read_json(&project_path), project_before);
    assert_eq!(read_json(&lock_path), lock_before);
    assert_eq!(read_json(&resources_path), resources_before);
    assert_eq!(read_json(&artifact_path), artifact_before);
    assert_eq!(
        fs::read_to_string(&sentinel).unwrap(),
        "keep this content\n"
    );
    assert_eq!(
        fs::read_dir(&external_home)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>(),
        vec![std::ffi::OsString::from("sentinel.txt")]
    );

    fs::remove_dir_all(&root).ok();
    fs::remove_dir_all(&external_home).ok();
}

/// Fail mode: an invocation-log write failure is promoted to a business
/// failure, replacing the unchanged verify result or exit code.
#[test]
fn invocation_log_failure_is_isolated_warning() {
    let root = setup("draft", "log-warning");
    fs::write(root.join(".appsdk-control"), "blocked\n").unwrap();

    let output = verify(&root, false);
    assert!(
        output.status.success(),
        "log failure must not change the verify result; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["command_ok"], json!(true));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("COMMAND_INVOCATION_LOG_WRITE_FAILED"),
        "log failure must surface as a warning, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(&root).ok();
}
