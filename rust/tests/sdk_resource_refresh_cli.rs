use serde_json::{json, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(unix)]
use std::os::unix::fs::symlink;

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

fn write_file(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn create_project(name: &str, core_version: &str) -> PathBuf {
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
    root
}

fn refresh(root: &Path) -> Output {
    run_for(root, &["resources", "refresh", root.to_str().unwrap()])
}

fn cleanup(root: &Path) {
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(global_registry_root(root));
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
fn resource_refresh_updates_selected_resources_and_schema2_record_is_readable() {
    let root = create_project("normal", "0.1.7");
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
    assert_eq!(record["schema_version"], json!(2));
    assert_eq!(record["version"], json!("0.1.7"));
    assert_eq!(record["resource_version"], json!(env!("APPSDK_VERSION")));
    assert!(!record.to_string().contains("digest"));

    let repeated = refresh(&root);
    assert!(
        repeated.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&repeated.stdout),
        String::from_utf8_lossy(&repeated.stderr)
    );
    assert_refreshed_resources(&root);
    assert_preserved_truth(&root, &project_before, &lock_before, &contract_before);
    assert_eq!(read_json(&root.join(".appsdk/sdk-resources.json")), record);
    cleanup(&root);
}

#[test]
fn resource_refresh_uses_structure_not_core_version_and_preserves_legacy_contract() {
    let root = create_project("legacy", "0.1.7");
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
    assert_eq!(record["version"], json!("0.1.7"));
    assert!(record["resources"].as_array().unwrap().iter().any(|entry| {
        entry["source"] == "contracts/records/evidence-record.schema.json"
            && entry.get("digest").is_none()
    }));

    let other = create_project("other-version", "0.1.8");
    let other_refreshed = refresh(&other);
    assert!(
        other_refreshed.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&other_refreshed.stdout),
        String::from_utf8_lossy(&other_refreshed.stderr)
    );
    assert_eq!(
        read_json(&other.join(".appsdk/sdk-resources.json"))["version"],
        json!("0.1.8")
    );
    cleanup(&root);
    cleanup(&other);
}

#[test]
fn resource_refresh_preflights_every_target_and_rejects_invalid_baseline() {
    let root = create_project("preflight", "0.1.7");
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
    cleanup(&root);
}

#[test]
fn resource_refresh_preserves_project_guidance_for_existing_consumers() {
    let root = create_project("guidance", "0.1.7");
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
    cleanup(&root);
}
