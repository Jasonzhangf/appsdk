use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

struct TestRoot {
    path: PathBuf,
}

impl Deref for TestRoot {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        #[cfg(unix)]
        restore_directory_permissions(&self.path);
        let _ = fs::remove_dir_all(&self.path);
        let _ = fs::remove_dir_all(global_registry_root(&self.path));
    }
}

#[cfg(unix)]
fn restore_directory_permissions(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_dir() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                restore_directory_permissions(&entry.path());
            }
        }
    }
}

fn binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_appsdk"))
}

fn temp_root(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    env::temp_dir().join(format!(
        "appsdk-resource-refresh-{name}-{}-{nonce}",
        std::process::id()
    ))
}

fn global_registry_root(root: &Path) -> PathBuf {
    root.with_extension("appsdk-home")
}

fn run_for(root: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .current_dir(root)
        .args(args)
        .env("APPSDK_HOME", global_registry_root(root))
        .env("COLLAB_STATE_DIR", root.join(".test-collab-state"))
        .env_remove("TMUX_PANE")
        .output()
        .unwrap()
}

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(value).unwrap() + "\n").unwrap();
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn assert_sha256_digest(value: &Value) {
    let digest = value.as_str().unwrap();
    assert_eq!(digest.len(), 71, "{digest}");
    assert!(digest.starts_with("sha256:"), "{digest}");
    assert!(digest[7..].chars().all(|value| value.is_ascii_hexdigit()));
}

fn assert_schema1_record_matches_files(root: &Path) -> Value {
    let record = read_json(&root.join(".appsdk/sdk-resources.json"));
    assert_eq!(record["schema_version"], json!(1));
    assert_eq!(record["sdk"], json!("appsdk"));
    assert_eq!(record["version"], json!(env!("APPSDK_VERSION")));
    assert!(record.get("resource_version").is_none());
    assert_sha256_digest(&record["bundle_digest"]);
    assert_sha256_digest(&record["manifest_digest"]);
    let entries = record["resources"].as_array().unwrap();
    assert!(!entries.is_empty());
    for entry in entries {
        let digest = entry["digest"].as_str().unwrap();
        let target = root.join(entry["path"].as_str().unwrap());
        assert_eq!(digest, sha256_bytes(&fs::read(target).unwrap()));
    }
    record
}

fn write_file(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn new_project(name: &str) -> TestRoot {
    let root = temp_root(name);
    fs::create_dir_all(&root).unwrap();
    let created = run_for(&root, &["new", root.to_str().unwrap()]);
    assert!(
        created.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr)
    );
    TestRoot { path: root }
}

fn create_project(name: &str, core_version: &str) -> TestRoot {
    let root = temp_root(name);
    for directory in [
        ".appsdk/contracts/records",
        ".appsdk/contracts/transitions",
        ".appsdk/guidance",
        ".appsdk/maps",
        ".appsdk/records",
        ".appsdk/migrations",
        ".appsdk/docs/design",
        ".appsdk/rules",
        ".appsdk/skills/appsdk-project-governance",
        ".appsdk/skills/project-memory",
        "active/lib",
        "protected/history",
        "source",
    ] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }

    write_json(
        &root.join(".appsdk/project.json"),
        &json!({
            "schema_version": 1,
            "project_id": format!("resource-refresh-{name}"),
            "sdk": {
                "name": "appsdk",
                "version": core_version,
                "bundle_manifest": ".appsdk/contracts/sdk-bundle.manifest.json",
                "resource_record": ".appsdk/sdk-resources.json"
            },
            "lifecycle": {"stage": "source_implemented"},
            "access": {"protected_paths": ["protected/**"]},
            "development_scenarios": {
                "manifest": ".appsdk/contracts/development-scenarios.manifest.json",
                "enabled": []
            },
            "guidance": {
                "enforcement": "advisory",
                "compiled_manifest": ".appsdk/guidance/compiled.json",
                "rule_sources": [{
                    "source_id": "project-governance",
                    "kind": "skill",
                    "path": ".appsdk/skills/appsdk-project-governance/SKILL.md",
                    "contract_path": ".appsdk/guidance/project-guidance.json",
                    "required": true,
                    "precedence": 100
                }]
            },
            "governance": {
                "playground_root": "playground",
                "active_root": "active",
                "protected_root": "protected",
                "generated_root": "generated",
                "active_kind": "immutable_consumable_library",
                "protected_kinds": ["history"],
                "generated_kinds": ["library"],
                "freeze_requirements": [],
                "promotion_requires": [],
                "runtime_forbidden_roots": ["generated"],
                "record_contracts": [".appsdk/contracts/records/evidence-record.schema.json"],
                "zone_transition_contract": ".appsdk/contracts/transitions/zone-transition.manifest.json",
                "playground_retention": "archive_then_remove",
                "debug_merge_comment_required": true
            },
            "modules": [{
                "module_id": "app-core",
                "stage": "source_implemented",
                "owned_paths": ["source/**"],
                "source_owner": "app-core",
                "active_artifact": "active/lib/app-core/**",
                "generated_outputs": ["generated/**"],
                "contract_paths": [".appsdk/contracts/**"],
                "dependency_modules": [],
                "build": {"program": "true", "args": [], "working_directory": "."},
                "artifact_paths": ["app-core.placeholder"]
            }]
        }),
    );

    write_json(
        &root.join(".appsdk/contracts/sdk-bundle.manifest.json"),
        &json!({
            "schema_version": 1,
            "sdk": "appsdk",
            "version": core_version,
            "runtime_entrypoint": "rust-binary",
            "resources": {
                "contracts": ["contracts/records/evidence-record.schema.json"],
                "docs": ["docs/design/appsdk-project-integration.md"],
                "rules": ["skills/appsdk-project-governance/SKILL.md"],
                "skills": [
                    "skills/appsdk-project-governance/SKILL.md",
                    "skills/project-memory/SKILL.md"
                ]
            },
            "install": {
                "project_roots": {
                    "contracts": ".appsdk/contracts",
                    "docs": ".appsdk/docs",
                    "rules": ".appsdk/rules",
                    "skills": ".appsdk/skills"
                },
                "generated_record": ".appsdk/sdk-resources.json"
            }
        }),
    );

    write_json(
        &root.join(".appsdk/sdk-resources.json"),
        &json!({
            "schema_version": 1,
            "sdk": "appsdk",
            "version": core_version,
            "bundle_digest": "opaque historical bundle identity",
            "manifest_digest": "opaque historical manifest identity",
            "resources": [
                {
                    "source": "contracts/records/evidence-record.schema.json",
                    "class": "contracts",
                    "path": ".appsdk/contracts/records/evidence-record.schema.json",
                    "digest": "opaque historical contract identity"
                },
                {
                    "source": "docs/design/appsdk-project-integration.md",
                    "class": "docs",
                    "path": ".appsdk/docs/design/appsdk-project-integration.md",
                    "digest": "opaque historical docs identity"
                },
                {
                    "source": "skills/appsdk-project-governance/SKILL.md",
                    "class": "rules",
                    "path": ".appsdk/rules/appsdk-project-governance.md",
                    "digest": "opaque historical rules identity"
                },
                {
                    "source": "skills/appsdk-project-governance/SKILL.md",
                    "class": "skills",
                    "path": ".appsdk/skills/appsdk-project-governance/SKILL.md",
                    "digest": "opaque historical skill identity"
                },
                {
                    "source": "skills/project-memory/SKILL.md",
                    "class": "skills",
                    "path": ".appsdk/skills/project-memory/SKILL.md",
                    "digest": "opaque historical memory skill identity"
                }
            ]
        }),
    );

    write_json(
        &root.join(".appsdk/contracts/records/evidence-record.schema.json"),
        &json!({"schema_version": 1, "fixture_contract": "preserved"}),
    );
    write_json(
        &root.join(".appsdk/contracts/development-scenarios.manifest.json"),
        &json!({"schema_version": 1, "scenarios": []}),
    );
    write_json(
        &root.join(".appsdk/contracts/transitions/zone-transition.manifest.json"),
        &json!({"schema_version": 1, "fixture_contract": "preserved"}),
    );
    write_json(
        &root.join(".appsdk/guidance/project-guidance.json"),
        &json!({
            "schema_version": 1,
            "skill_id": "project-governance",
            "guidance_version": "1",
            "rules": [{
                "rule_id": "project-rule",
                "severity": "warning",
                "description": "Keep the project-owned guidance contract."
            }],
            "workflows": [{
                "domain": "governance-preflight",
                "objective": "Consume the project-owned guidance contract.",
                "entry_node": "orient",
                "nodes": [{
                    "node_id": "orient",
                    "instruction": "Use the project-owned workflow.",
                    "owner": "app-core",
                    "evidence": ["project-evidence"]
                }],
                "edges": []
            }]
        }),
    );
    write_json(
        &root.join(".appsdk/maps/resource-map.json"),
        &json!({"schema_version": 1, "map_sentinel": "preserved"}),
    );
    write_json(
        &root.join(".appsdk/records/project-record.json"),
        &json!({"fixture_record": "preserved"}),
    );
    write_json(
        &root.join(".appsdk/migrations/history.json"),
        &json!({"fixture_history": "preserved"}),
    );
    write_json(
        &root.join(".appsdk/sdk.lock"),
        &json!({
            "sdk": "appsdk",
            "version": core_version,
            "contract_schema": 1,
            "lock_sentinel": "preserved"
        }),
    );
    write_json(
        &root.join(".appsdk/requirements.json"),
        &json!({"fixture_requirements": "preserved"}),
    );
    write_json(
        &root.join(".appsdk/goal.json"),
        &json!({"fixture_goal": "preserved"}),
    );

    for (relative, content) in [
        ("AGENTS.md", "# Project rules\n"),
        ("requirements.md", "fixture requirements\n"),
        ("goal.md", "fixture goal\n"),
        ("source/app.txt", "source\n"),
        (".appsdk/records/business-sentinel.txt", "business\n"),
        ("active/lib/project-active.txt", "active\n"),
        ("protected/history/project-history.txt", "protected\n"),
        (
            ".appsdk/docs/design/appsdk-project-integration.md",
            "legacy docs\n",
        ),
        (
            ".appsdk/rules/appsdk-project-governance.md",
            "legacy rules\n",
        ),
        (
            ".appsdk/skills/appsdk-project-governance/SKILL.md",
            "legacy project-governance skill\n",
        ),
        (
            ".appsdk/skills/project-memory/SKILL.md",
            "legacy project-memory skill\n",
        ),
    ] {
        write_file(&root, relative, content);
    }
    TestRoot { path: root }
}

fn refresh(root: &Path) -> Output {
    run_for(root, &["resources", "refresh", root.to_str().unwrap()])
}

fn assert_refreshed_resources(root: &Path) {
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/docs/design/appsdk-project-integration.md")).unwrap(),
        include_str!("../../docs/design/appsdk-project-integration.md")
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/rules/appsdk-project-governance.md")).unwrap(),
        include_str!("../../sdk-skill-sources/appsdk-project-governance/SKILL.md")
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/skills/appsdk-project-governance/SKILL.md")).unwrap(),
        include_str!("../../sdk-skill-sources/appsdk-project-governance/SKILL.md")
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/skills/project-memory/SKILL.md")).unwrap(),
        include_str!("../../sdk-skill-sources/project-memory/SKILL.md")
    );
}

fn assert_preserved_truth(root: &Path, project: &[u8], lock: &[u8], contract: &[u8]) {
    assert_eq!(
        fs::read(root.join(".appsdk/project.json")).unwrap(),
        project
    );
    assert_eq!(fs::read(root.join(".appsdk/sdk.lock")).unwrap(), lock);
    assert_eq!(
        fs::read(root.join(".appsdk/contracts/records/evidence-record.schema.json")).unwrap(),
        contract
    );
    assert_eq!(
        fs::read_to_string(root.join(".appsdk/records/business-sentinel.txt")).unwrap(),
        "business\n"
    );
    assert_eq!(
        read_json(&root.join(".appsdk/maps/resource-map.json"))["map_sentinel"],
        json!("preserved")
    );
    assert_eq!(
        fs::read_to_string(root.join("active/lib/project-active.txt")).unwrap(),
        "active\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("protected/history/project-history.txt")).unwrap(),
        "protected\n"
    );
}

#[test]
fn resource_refresh_writes_schema1_record_with_content_digests() {
    // Catches the old writer publishing schema 2 without non-contract digests.
    let root = create_project("normal", env!("APPSDK_VERSION"));
    let project_before = fs::read(root.join(".appsdk/project.json")).unwrap();
    let lock_before = fs::read(root.join(".appsdk/sdk.lock")).unwrap();
    let contract_before = b"opaque historical contract bytes\n".to_vec();
    fs::write(
        root.join(".appsdk/contracts/records/evidence-record.schema.json"),
        &contract_before,
    )
    .unwrap();

    let refreshed = refresh(&root);
    assert!(
        refreshed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&refreshed.stdout),
        String::from_utf8_lossy(&refreshed.stderr)
    );
    assert_refreshed_resources(&root);
    assert_preserved_truth(&root, &project_before, &lock_before, &contract_before);
    let record = read_json(&root.join(".appsdk/sdk-resources.json"));
    assert_eq!(record["schema_version"], json!(1));
    assert_eq!(record["version"], json!(env!("APPSDK_VERSION")));
    assert!(record.get("resource_version").is_none());
    assert_sha256_digest(&record["bundle_digest"]);
    assert_sha256_digest(&record["manifest_digest"]);
    assert_eq!(
        record["resources"][0]["digest"],
        json!("opaque historical contract identity")
    );
    for entry in record["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["class"] != "contracts")
    {
        let digest = entry["digest"].as_str().unwrap();
        let target = root.join(entry["path"].as_str().unwrap());
        assert_eq!(digest, sha256_bytes(&fs::read(target).unwrap()));
    }
}

#[test]
fn resource_refresh_then_public_verify_uses_one_schema1_contract() {
    // Catches the RED path where refresh succeeds with schema 2 and verify rejects it.
    let root = new_project("refresh-verify");
    let initial = read_json(&root.join(".appsdk/sdk-resources.json"));
    let initial_bundle_digest = initial["bundle_digest"].clone();
    let manifest_digest =
        sha256_bytes(&fs::read(root.join(".appsdk/contracts/sdk-bundle.manifest.json")).unwrap());

    let refreshed = refresh(&root);
    assert!(
        refreshed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&refreshed.stdout),
        String::from_utf8_lossy(&refreshed.stderr)
    );
    let record = assert_schema1_record_matches_files(&root);
    assert_eq!(record["bundle_digest"], initial_bundle_digest);
    assert_eq!(record["manifest_digest"], json!(manifest_digest));

    let verified = run_for(&root, &["verify", root.to_str().unwrap()]);
    assert!(
        verified.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );

    let record_before = fs::read(root.join(".appsdk/sdk-resources.json")).unwrap();
    let repeated = refresh(&root);
    assert!(
        repeated.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&repeated.stdout),
        String::from_utf8_lossy(&repeated.stderr)
    );
    assert_eq!(
        fs::read(root.join(".appsdk/sdk-resources.json")).unwrap(),
        record_before
    );
    let verified_again = run_for(&root, &["verify", root.to_str().unwrap()]);
    assert!(
        verified_again.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&verified_again.stdout),
        String::from_utf8_lossy(&verified_again.stderr)
    );
}

#[test]
fn resource_refresh_rejects_old_core_pin_before_writing() {
    let root = new_project("old-core-pin");
    let project_path = root.join(".appsdk/project.json");
    let manifest_path = root.join(".appsdk/contracts/sdk-bundle.manifest.json");
    let record_path = root.join(".appsdk/sdk-resources.json");

    let mut project = read_json(&project_path);
    project["sdk"]["version"] = json!("0.1.0006");
    write_json(&project_path, &project);
    let mut manifest = read_json(&manifest_path);
    manifest["version"] = json!("0.1.0006");
    write_json(&manifest_path, &manifest);
    let mut record = read_json(&record_path);
    record["version"] = json!("0.1.0006");
    write_json(&record_path, &record);

    let record_before = fs::read(&record_path).unwrap();
    let mut target_before = Vec::new();
    for entry in record["resources"].as_array().unwrap() {
        if entry["class"] != "contracts" {
            let target = root.join(entry["path"].as_str().unwrap());
            target_before.push(fs::read(&target).unwrap());
        }
    }

    let rejected = refresh(&root);
    assert!(
        !rejected.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("PROJECT_SDK_VERSION_PIN_MISMATCH:0.1.0006:required_binary=appsdk-0.1.0006"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(fs::read(&record_path).unwrap(), record_before);
    let mut target_after = Vec::new();
    for entry in record["resources"].as_array().unwrap() {
        if entry["class"] != "contracts" {
            let target = root.join(entry["path"].as_str().unwrap());
            target_after.push(fs::read(&target).unwrap());
        }
    }
    assert_eq!(target_after, target_before);

    let verified = run_for(&root, &["verify", root.to_str().unwrap()]);
    assert!(
        String::from_utf8_lossy(&verified.stderr)
            .contains("PROJECT_SDK_VERSION_PIN_MISMATCH:0.1.0006:required_binary=appsdk-0.1.0006"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
}

#[test]
fn refresh_migrates_legacy_schema2_input_to_schema1_and_verify_accepts_it() {
    // Catches a migration that only changes schema_version without restoring digest coverage.
    let root = new_project("schema2-migration");
    let record_path = root.join(".appsdk/sdk-resources.json");
    let mut legacy = read_json(&record_path);
    legacy["schema_version"] = json!(2);
    legacy["resource_version"] = json!(env!("APPSDK_VERSION"));
    legacy.as_object_mut().unwrap().remove("bundle_digest");
    legacy.as_object_mut().unwrap().remove("manifest_digest");
    for entry in legacy["resources"].as_array_mut().unwrap() {
        if entry["class"] != "contracts" {
            entry.as_object_mut().unwrap().remove("digest");
        }
    }
    write_json(&record_path, &legacy);

    let refreshed = refresh(&root);
    assert!(
        refreshed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&refreshed.stdout),
        String::from_utf8_lossy(&refreshed.stderr)
    );
    assert_schema1_record_matches_files(&root);
    let verified = run_for(&root, &["verify", root.to_str().unwrap()]);
    assert!(
        verified.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&verified.stdout),
        String::from_utf8_lossy(&verified.stderr)
    );
}

#[test]
fn resource_refresh_preserves_legacy_contract_for_current_core_pin() {
    let root = create_project("legacy", env!("APPSDK_VERSION"));
    let project_before = fs::read(root.join(".appsdk/project.json")).unwrap();
    let lock_before = fs::read(root.join(".appsdk/sdk.lock")).unwrap();
    let contract_before =
        fs::read(root.join(".appsdk/contracts/records/evidence-record.schema.json")).unwrap();

    let refreshed = refresh(&root);
    assert!(
        refreshed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&refreshed.stdout),
        String::from_utf8_lossy(&refreshed.stderr)
    );
    assert_refreshed_resources(&root);
    assert_preserved_truth(&root, &project_before, &lock_before, &contract_before);
    let record = read_json(&root.join(".appsdk/sdk-resources.json"));
    assert_eq!(record["version"], json!(env!("APPSDK_VERSION")));
    assert!(record["resources"].as_array().unwrap().iter().any(|entry| {
        entry["source"] == "contracts/records/evidence-record.schema.json"
            && entry["digest"] == "opaque historical contract identity"
    }));
}

#[test]
fn resource_refresh_preflights_every_target_and_rejects_invalid_baseline() {
    let root = create_project("preflight", env!("APPSDK_VERSION"));
    let first_target = root.join(".appsdk/docs/design/appsdk-project-integration.md");
    let first_before = fs::read(&first_target).unwrap();
    let record_before = fs::read(root.join(".appsdk/sdk-resources.json")).unwrap();

    #[cfg(unix)]
    {
        let outside = root.join("outside.md");
        fs::write(&outside, "outside\n").unwrap();
        let later_target = root.join(".appsdk/skills/project-memory/SKILL.md");
        fs::remove_file(&later_target).unwrap();
        symlink(&outside, &later_target).unwrap();

        let rejected = refresh(&root);
        assert!(!rejected.status.success());
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("GOVERNANCE_PATH_SYMLINK:"));
        assert_eq!(fs::read(&first_target).unwrap(), first_before);
        assert_eq!(
            fs::read(root.join(".appsdk/sdk-resources.json")).unwrap(),
            record_before
        );
        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside\n");
        fs::remove_file(&later_target).unwrap();
        write_file(
            &root,
            ".appsdk/skills/project-memory/SKILL.md",
            "legacy memory skill\n",
        );
    }

    let record_path = root.join(".appsdk/sdk-resources.json");
    let mut invalid_record = read_json(&record_path);
    invalid_record["schema_version"] = json!(99);
    write_json(&record_path, &invalid_record);
    let invalid = refresh(&root);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("INVALID_SDK_RESOURCES"));
    assert_eq!(fs::read(&first_target).unwrap(), first_before);
}

#[test]
fn resource_refresh_preserves_project_guidance_for_existing_consumers() {
    let root = create_project("guidance", env!("APPSDK_VERSION"));
    let contract_before = fs::read(root.join(".appsdk/guidance/project-guidance.json")).unwrap();
    let refreshed = refresh(&root);
    assert!(
        refreshed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&refreshed.stdout),
        String::from_utf8_lossy(&refreshed.stderr)
    );
    assert_eq!(
        fs::read(root.join(".appsdk/guidance/project-guidance.json")).unwrap(),
        contract_before
    );

    let compiled = run_for(&root, &["guide", "compile", root.to_str().unwrap()]);
    assert!(
        compiled.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );
    let compiled_value: Value = serde_json::from_slice(&compiled.stdout).unwrap();
    assert_eq!(compiled_value["rules"][0]["rule_id"], json!("project-rule"));
    assert_eq!(
        compiled_value["sources"][0]["contract_path"],
        json!(".appsdk/guidance/project-guidance.json")
    );

    let status = run_for(&root, &["guide", "status", root.to_str().unwrap()]);
    assert!(
        status.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&status.stdout),
        String::from_utf8_lossy(&status.stderr)
    );
    let status_value: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_value["domain"], json!("governance-preflight"));
    assert_eq!(status_value["next"]["node_id"], json!("orient"));

    let mut invalid_contract = read_json(&root.join(".appsdk/guidance/project-guidance.json"));
    invalid_contract["schema_version"] = json!(2);
    write_json(
        &root.join(".appsdk/guidance/project-guidance.json"),
        &invalid_contract,
    );
    let rejected = run_for(&root, &["guide", "compile", root.to_str().unwrap()]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("GUIDANCE_MANIFEST_SCHEMA_INVALID"));
}

#[cfg(unix)]
#[test]
fn resource_refresh_reports_partial_write_and_does_not_publish_record() {
    let effective_uid = Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|uid| uid.trim().parse::<u32>().ok());
    if effective_uid == Some(0) {
        eprintln!("UNVERIFIED: SDK resource write failure requires a non-root Unix user");
        return;
    }

    let root = create_project("write-failure", env!("APPSDK_VERSION"));
    let record_path = root.join(".appsdk/sdk-resources.json");
    let record_before = fs::read(&record_path).unwrap();
    let blocked_directory = root.join(".appsdk/skills/project-memory");
    let original_permissions = fs::metadata(&blocked_directory).unwrap().permissions();
    fs::set_permissions(&blocked_directory, fs::Permissions::from_mode(0o500)).unwrap();

    let failed = refresh(&root);
    let stdout = String::from_utf8_lossy(&failed.stdout);
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(!failed.status.success(), "stdout={stdout} stderr={stderr}");
    assert!(
        stderr.contains("SDK_RESOURCE_WRITE_FAILED"),
        "stdout={stdout} stderr={stderr}"
    );
    assert!(
        stdout.contains("refreshed .appsdk/docs/"),
        "stdout={stdout} stderr={stderr}"
    );
    assert!(
        !stdout.contains("resource refresh complete:"),
        "stdout={stdout} stderr={stderr}"
    );
    assert_eq!(fs::read(&record_path).unwrap(), record_before);

    fs::set_permissions(&blocked_directory, original_permissions).unwrap();
    let retried = refresh(&root);
    assert!(
        retried.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&retried.stdout),
        String::from_utf8_lossy(&retried.stderr)
    );
    assert_refreshed_resources(&root);
    assert_eq!(read_json(&record_path)["schema_version"], json!(1));
}
