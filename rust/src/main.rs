use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const SDK_BUNDLE_MANIFEST: &str = include_str!("../../contracts/sdk-bundle.manifest.json");
const SDK_MAP_MIGRATION_MANIFEST: &str =
    include_str!("../../contracts/migrations/sdk-0.1.5-to-0.1.6.json");
const GOVERNANCE_MAP_NAMES: [&str; 4] = [
    "resource-map.json",
    "function-map.json",
    "mainline-call-map.json",
    "verification-map.json",
];
const SDK_BUNDLE_RESOURCES: &[(&str, &str, &str)] = &[
    (
        "contracts/sdk-bundle.manifest.json",
        "contracts",
        include_str!("../../contracts/sdk-bundle.manifest.json"),
    ),
    (
        "contracts/project.schema.json",
        "contracts",
        include_str!("../../contracts/project.schema.json"),
    ),
    (
        "contracts/development-scenarios.manifest.json",
        "contracts",
        include_str!("../../contracts/development-scenarios.manifest.json"),
    ),
    (
        "contracts/maps/resource-map.json",
        "contracts",
        include_str!("../../contracts/maps/resource-map.json"),
    ),
    (
        "contracts/maps/function-map.json",
        "contracts",
        include_str!("../../contracts/maps/function-map.json"),
    ),
    (
        "contracts/maps/mainline-call-map.json",
        "contracts",
        include_str!("../../contracts/maps/mainline-call-map.json"),
    ),
    (
        "contracts/maps/verification-map.json",
        "contracts",
        include_str!("../../contracts/maps/verification-map.json"),
    ),
    (
        "contracts/migrations/sdk-0.1.5-to-0.1.6.json",
        "contracts",
        include_str!("../../contracts/migrations/sdk-0.1.5-to-0.1.6.json"),
    ),
    (
        "contracts/migrations/0.1.5/governance-maps/resource-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.5/governance-maps/resource-map.json"),
    ),
    (
        "contracts/migrations/0.1.5/governance-maps/function-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.5/governance-maps/function-map.json"),
    ),
    (
        "contracts/migrations/0.1.5/governance-maps/mainline-call-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.5/governance-maps/mainline-call-map.json"),
    ),
    (
        "contracts/migrations/0.1.5/governance-maps/verification-map.json",
        "contracts",
        include_str!("../../contracts/migrations/0.1.5/governance-maps/verification-map.json"),
    ),
    (
        "contracts/transitions/zone-transition.manifest.json",
        "contracts",
        include_str!("../../contracts/transitions/zone-transition.manifest.json"),
    ),
    (
        "contracts/lifecycle-state-machines.manifest.json",
        "contracts",
        include_str!("../../contracts/lifecycle-state-machines.manifest.json"),
    ),
    (
        "contracts/records/record-graph.contract.json",
        "contracts",
        include_str!("../../contracts/records/record-graph.contract.json"),
    ),
    (
        "contracts/records/worktree-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/worktree-record.schema.json"),
    ),
    (
        "contracts/records/reproduction-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/reproduction-record.schema.json"),
    ),
    (
        "contracts/records/evidence-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/evidence-record.schema.json"),
    ),
    (
        "contracts/records/review-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/review-record.schema.json"),
    ),
    (
        "contracts/records/fix-candidate-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/fix-candidate-record.schema.json"),
    ),
    (
        "contracts/records/effectiveness-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/effectiveness-record.schema.json"),
    ),
    (
        "contracts/records/pre-review-validation-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/pre-review-validation-record.schema.json"),
    ),
    (
        "contracts/records/collaboration-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/collaboration-record.schema.json"),
    ),
    (
        "contracts/records/collaboration-index.schema.json",
        "contracts",
        include_str!("../../contracts/records/collaboration-index.schema.json"),
    ),
    (
        "contracts/records/merge-queue-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/merge-queue-record.schema.json"),
    ),
    (
        "contracts/records/merge-queue-state.schema.json",
        "contracts",
        include_str!("../../contracts/records/merge-queue-state.schema.json"),
    ),
    (
        "contracts/records/integration-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/integration-record.schema.json"),
    ),
    (
        "contracts/records/mainline-receipt-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/mainline-receipt-record.schema.json"),
    ),
    (
        "contracts/records/merge-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/merge-record.schema.json"),
    ),
    (
        "contracts/records/promotion-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/promotion-record.schema.json"),
    ),
    (
        "contracts/records/freeze-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/freeze-record.schema.json"),
    ),
    (
        "contracts/records/regression-report.schema.json",
        "contracts",
        include_str!("../../contracts/records/regression-report.schema.json"),
    ),
    (
        "docs/design/appsdk-project-integration.md",
        "docs",
        include_str!("../../docs/design/appsdk-project-integration.md"),
    ),
    (
        "docs/design/fix-lifecycle-v2.md",
        "docs",
        include_str!("../../docs/design/fix-lifecycle-v2.md"),
    ),
    (
        "docs/design/development-scenarios.md",
        "docs",
        include_str!("../../docs/design/development-scenarios.md"),
    ),
    (
        "docs/architecture/appsdk-governance-architecture.md",
        "docs",
        include_str!("../../docs/architecture/appsdk-governance-architecture.md"),
    ),
    (
        "skills/appsdk-project-governance/SKILL.md",
        "rules",
        include_str!("../../skills/appsdk-project-governance/SKILL.md"),
    ),
    (
        "skills/appsdk-project-governance/SKILL.md",
        "skills",
        include_str!("../../skills/appsdk-project-governance/SKILL.md"),
    ),
];

fn canonical_governance_map(name: &str) -> &'static str {
    match name {
        "resource-map.json" => include_str!("../../contracts/maps/resource-map.json"),
        "function-map.json" => include_str!("../../contracts/maps/function-map.json"),
        "mainline-call-map.json" => include_str!("../../contracts/maps/mainline-call-map.json"),
        "verification-map.json" => include_str!("../../contracts/maps/verification-map.json"),
        _ => fail("UNKNOWN_GOVERNANCE_MAP"),
    }
}

fn historical_governance_map(name: &str) -> &'static str {
    match name {
        "resource-map.json" => {
            include_str!("../../contracts/migrations/0.1.5/governance-maps/resource-map.json")
        }
        "function-map.json" => {
            include_str!("../../contracts/migrations/0.1.5/governance-maps/function-map.json")
        }
        "mainline-call-map.json" => {
            include_str!("../../contracts/migrations/0.1.5/governance-maps/mainline-call-map.json")
        }
        "verification-map.json" => {
            include_str!("../../contracts/migrations/0.1.5/governance-maps/verification-map.json")
        }
        _ => fail("UNKNOWN_GOVERNANCE_MAP"),
    }
}

fn sdk_map_migration_manifest() -> Value {
    let manifest: Value = serde_json::from_str(SDK_MAP_MIGRATION_MANIFEST)
        .unwrap_or_else(|_| fail("INVALID_SDK_MAP_MIGRATION_MANIFEST"));
    if manifest.get("schema_version").and_then(Value::as_u64) != Some(1)
        || manifest.get("migration_id").and_then(Value::as_str) != Some("appsdk-0.1.5-to-0.1.6")
        || manifest.get("source_version").and_then(Value::as_str) != Some("0.1.5")
        || manifest.get("target_version").and_then(Value::as_str) != Some("0.1.6")
        || manifest.get("snapshot_root").and_then(Value::as_str)
            != Some(".appsdk/migrations/0.1.5-to-0.1.6/maps")
        || manifest.get("record_path").and_then(Value::as_str)
            != Some(".appsdk/migrations/0.1.5-to-0.1.6/record.json")
    {
        fail("INVALID_SDK_MAP_MIGRATION_MANIFEST");
    }
    let maps = manifest
        .get("maps")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_SDK_MAP_MIGRATION_MANIFEST"));
    if maps.len() != GOVERNANCE_MAP_NAMES.len() {
        fail("INVALID_SDK_MAP_MIGRATION_MANIFEST");
    }
    for name in GOVERNANCE_MAP_NAMES {
        let entry = maps
            .iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| fail("INVALID_SDK_MAP_MIGRATION_MANIFEST"));
        if entry.get("source_digest").and_then(Value::as_str)
            != Some(digest_bytes(historical_governance_map(name).as_bytes()).as_str())
            || entry.get("target_digest").and_then(Value::as_str)
                != Some(digest_bytes(canonical_governance_map(name).as_bytes()).as_str())
        {
            fail(format!(
                "SDK_MAP_MIGRATION_MANIFEST_DIGEST_MISMATCH:{}",
                name
            ));
        }
    }
    manifest
}

fn sdk_bundle_manifest_resources() -> Value {
    serde_json::from_str::<Value>(SDK_BUNDLE_MANIFEST)
        .unwrap_or_else(|_| fail("INVALID_SDK_BUNDLE_MANIFEST"))
        .get("resources")
        .cloned()
        .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_MANIFEST"))
}

fn sdk_resource_install_relative(source: &str, class: &str) -> String {
    match class {
        "contracts" => format!(
            ".appsdk/contracts/{}",
            source
                .strip_prefix("contracts/")
                .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE"))
        ),
        "docs" => format!(
            ".appsdk/docs/{}",
            source
                .strip_prefix("docs/")
                .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE"))
        ),
        "rules" => ".appsdk/rules/appsdk-project-governance.md".into(),
        "skills" => ".appsdk/skills/appsdk-project-governance/SKILL.md".into(),
        _ => fail("INVALID_SDK_BUNDLE"),
    }
}

fn sdk_bundle_resource_entries() -> Vec<(String, String, &'static str)> {
    let resources = sdk_bundle_manifest_resources()
        .as_object()
        .cloned()
        .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_MANIFEST"));
    let mut entries = Vec::new();
    for (class, paths) in &resources {
        let paths = paths
            .as_array()
            .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_MANIFEST"));
        for path in paths {
            let source = path
                .as_str()
                .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_MANIFEST"));
            let content = SDK_BUNDLE_RESOURCES
                .iter()
                .find(|(embedded_source, embedded_class, _)| {
                    *embedded_source == source && *embedded_class == class
                })
                .map(|(_, _, content)| *content)
                .unwrap_or_else(|| fail(format!("SDK_BUNDLE_MANIFEST_MISMATCH:{}", source)));
            entries.push((source.to_string(), class.clone(), content));
        }
    }
    if entries.len() != SDK_BUNDLE_RESOURCES.len() {
        fail("SDK_BUNDLE_RESOURCE_SET_MISMATCH");
    }
    entries
}

fn sdk_bundle_digest() -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"manifest\0");
    hasher.update(SDK_BUNDLE_MANIFEST.as_bytes());
    for (path, class, content) in sdk_bundle_resource_entries() {
        hasher.update(b"resource\0");
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update(class.as_bytes());
        hasher.update(b"\0");
        hasher.update(content.as_bytes());
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn assert_bundle_manifest() {
    let manifest: Value = serde_json::from_str(SDK_BUNDLE_MANIFEST)
        .unwrap_or_else(|_| fail("INVALID_SDK_BUNDLE_MANIFEST"));
    if manifest.get("schema_version").and_then(Value::as_u64) != Some(1)
        || manifest.get("sdk").and_then(Value::as_str) != Some("appsdk")
        || manifest.get("version").and_then(Value::as_str) != Some("0.1.6")
        || manifest.get("runtime_entrypoint").and_then(Value::as_str) != Some("rust-binary")
    {
        fail("INVALID_SDK_BUNDLE_MANIFEST");
    }
    let _ = sdk_bundle_resource_entries();
}

fn install_bundle_resources(root: &Path) {
    assert_bundle_manifest();
    let mut installed = Vec::new();
    for (source, class, content) in sdk_bundle_resource_entries() {
        let target = root.join(sdk_resource_install_relative(&source, &class));
        assert_no_symlink_components(root, &target, "sdk_resource");
        if fs::symlink_metadata(&target)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(format!(
                "GOVERNANCE_PATH_SYMLINK:sdk_resource:{}",
                target.display()
            ));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|_| fail("SDK_RESOURCE_WRITE_FAILED"));
        }
        atomic_write_bytes(&target, content.as_bytes(), "SDK_RESOURCE_WRITE_FAILED");
        installed.push(serde_json::json!({
            "source": source,
            "class": class,
            "path": target.strip_prefix(root).unwrap().to_string_lossy(),
            "digest": digest_bytes(content.as_bytes())
        }));
    }
    let record = serde_json::json!({
        "schema_version": 1,
        "sdk": "appsdk",
        "version": "0.1.6",
        "bundle_digest": sdk_bundle_digest(),
        "manifest_digest": digest_bytes(SDK_BUNDLE_MANIFEST.as_bytes()),
        "resources": installed
    });
    let record_path = root.join(".appsdk/sdk-resources.json");
    atomic_write_json(&record_path, &record, "SDK_RESOURCE_RECORD_WRITE_FAILED");
}

fn fail(message: impl AsRef<str>) -> ! {
    eprintln!("{}", message.as_ref());
    std::process::exit(1);
}

fn write_embedded_contract(root: &Path, relative: &str, content: &str) {
    let target = root.join(relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    }
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:contract");
    }
    if target.exists() {
        return;
    }
    fs::write(target, content).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
}

fn bootstrap_contracts(root: &Path) {
    write_embedded_contract(
        root,
        ".appsdk/contracts/development-scenarios.manifest.json",
        include_str!("../../contracts/development-scenarios.manifest.json"),
    );
    write_embedded_contract(
        root,
        ".appsdk/maps/resource-map.json",
        include_str!("../../contracts/maps/resource-map.json"),
    );
    write_embedded_contract(
        root,
        ".appsdk/maps/function-map.json",
        include_str!("../../contracts/maps/function-map.json"),
    );
    write_embedded_contract(
        root,
        ".appsdk/maps/mainline-call-map.json",
        include_str!("../../contracts/maps/mainline-call-map.json"),
    );
    write_embedded_contract(
        root,
        ".appsdk/maps/verification-map.json",
        include_str!("../../contracts/maps/verification-map.json"),
    );
    write_embedded_contract(
        root,
        ".appsdk/maps/module-registry.json",
        r#"{
  "schema_version": 1,
  "modules": [{
    "module_id": "app-core",
    "status": "active",
    "owner": "app-core",
    "owned_paths": ["playground/experiments/**", "tests/core/**"],
    "forbidden_paths": ["active/lib/**", "protected/**", "generated/**"],
    "verification_gates": ["fix_lifecycle_graph", "mainline_merge_identity"]
  }]
}
"#,
    );
    write_embedded_contract(
        root,
        "contracts/transitions/zone-transition-manifest.json",
        include_str!("../../contracts/transitions/zone-transition.manifest.json"),
    );
    write_embedded_contract(
        root,
        "contracts/transitions/zone-transition.manifest.json",
        include_str!("../../contracts/transitions/zone-transition.manifest.json"),
    );
    for (relative, content) in [
        (
            "contracts/records/worktree-record.schema.json",
            include_str!("../../contracts/records/worktree-record.schema.json"),
        ),
        (
            "contracts/records/reproduction-record.schema.json",
            include_str!("../../contracts/records/reproduction-record.schema.json"),
        ),
        (
            "contracts/records/evidence-record.schema.json",
            include_str!("../../contracts/records/evidence-record.schema.json"),
        ),
        (
            "contracts/records/goal-clarification-record.schema.json",
            include_str!("../../contracts/records/goal-clarification-record.schema.json"),
        ),
        (
            "contracts/records/fix-candidate-record.schema.json",
            include_str!("../../contracts/records/fix-candidate-record.schema.json"),
        ),
        (
            "contracts/records/review-record.schema.json",
            include_str!("../../contracts/records/review-record.schema.json"),
        ),
        (
            "contracts/records/effectiveness-record.schema.json",
            include_str!("../../contracts/records/effectiveness-record.schema.json"),
        ),
        (
            "contracts/records/pre-review-validation-record.schema.json",
            include_str!("../../contracts/records/pre-review-validation-record.schema.json"),
        ),
        (
            "contracts/records/collaboration-record.schema.json",
            include_str!("../../contracts/records/collaboration-record.schema.json"),
        ),
        (
            "contracts/records/collaboration-index.schema.json",
            include_str!("../../contracts/records/collaboration-index.schema.json"),
        ),
        (
            "contracts/records/merge-queue-record.schema.json",
            include_str!("../../contracts/records/merge-queue-record.schema.json"),
        ),
        (
            "contracts/records/merge-queue-state.schema.json",
            include_str!("../../contracts/records/merge-queue-state.schema.json"),
        ),
        (
            "contracts/records/integration-record.schema.json",
            include_str!("../../contracts/records/integration-record.schema.json"),
        ),
        (
            "contracts/records/mainline-receipt-record.schema.json",
            include_str!("../../contracts/records/mainline-receipt-record.schema.json"),
        ),
        (
            "contracts/records/merge-record.schema.json",
            include_str!("../../contracts/records/merge-record.schema.json"),
        ),
        (
            "contracts/records/promotion-record.schema.json",
            include_str!("../../contracts/records/promotion-record.schema.json"),
        ),
        (
            "contracts/records/regression-report.schema.json",
            include_str!("../../contracts/records/regression-report.schema.json"),
        ),
        (
            "contracts/records/freeze-record.schema.json",
            include_str!("../../contracts/records/freeze-record.schema.json"),
        ),
        (
            "contracts/records/record-graph.contract.json",
            include_str!("../../contracts/records/record-graph.contract.json"),
        ),
        (
            "contracts/lifecycle-state-machines.json",
            include_str!("../../contracts/lifecycle-state-machines.json"),
        ),
        (
            "contracts/lifecycle-state-machines.manifest.json",
            include_str!("../../contracts/lifecycle-state-machines.manifest.json"),
        ),
        (
            "contracts/goal-clarification-state-machine.json",
            include_str!("../../contracts/goal-clarification-state-machine.json"),
        ),
    ] {
        write_embedded_contract(root, relative, content);
    }
}

fn project_file(root: &Path) -> PathBuf {
    root.join(".appsdk").join("project.json")
}

fn read_project(root: &Path) -> Value {
    let file = project_file(root);
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:project");
    }
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|_| fail(format!("PROJECT_CONTRACT_MISSING:{}", file.display())));
    serde_json::from_str(&text).unwrap_or_else(|_| fail("INVALID_PROJECT_CONTRACT"))
}

fn assert_project_root_safe(root: &Path) {
    for ancestor in root.ancestors() {
        if ancestor == Path::new("/tmp") || ancestor == Path::new("/var") {
            continue;
        }
        if fs::symlink_metadata(ancestor)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(format!("PROJECT_ROOT_SYMLINK:{}", ancestor.display()));
        }
    }
    if fs::symlink_metadata(root)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("PROJECT_ROOT_SYMLINK");
    }
    let resolved = root
        .canonicalize()
        .unwrap_or_else(|_| fail("PROJECT_ROOT_MISSING"));
    for ancestor in resolved.ancestors() {
        if fs::symlink_metadata(ancestor)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(format!("PROJECT_ROOT_SYMLINK:{}", ancestor.display()));
        }
    }
    assert_no_symlink_components(root, &root.join(".appsdk"), "appsdk_control");
}

fn freeze_record_name(module_id: &str) -> String {
    format!("freeze-record-{}.json", module_id)
}

fn module_record_name(kind: &str, module_id: &str) -> String {
    format!("{}-{}.json", kind, module_id)
}

fn assert_version(value: &str, error: &str) {
    if value.is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        || value == "."
        || value == ".."
    {
        fail(error);
    }
}

fn assert_declared_contracts(root: &Path, project: &Value, strict: bool) {
    let zone = contract_root(root, project, "/governance/zone_transition_contract");
    let canonical_project = project
        .pointer("/governance/zone_transition_contract")
        .and_then(Value::as_str)
        == Some("contracts/transitions/zone-transition-manifest.json")
        && project
            .pointer("/governance/record_contracts")
            .and_then(Value::as_array)
            .map(|values| {
                values.iter().any(|value| {
                    value.as_str() == Some("contracts/records/record-graph.contract.json")
                })
            })
            .unwrap_or(false);
    let strict = strict || canonical_project;
    if !canonical_project {
        fail("NON_CANONICAL_GOVERNANCE_CONTRACT");
    }
    let canonical_records = [
        "contracts/records/worktree-record.schema.json",
        "contracts/records/reproduction-record.schema.json",
        "contracts/records/evidence-record.schema.json",
        "contracts/records/fix-candidate-record.schema.json",
        "contracts/records/goal-clarification-record.schema.json",
        "contracts/records/review-record.schema.json",
        "contracts/records/effectiveness-record.schema.json",
        "contracts/records/pre-review-validation-record.schema.json",
        "contracts/records/collaboration-record.schema.json",
        "contracts/records/collaboration-index.schema.json",
        "contracts/records/merge-queue-record.schema.json",
        "contracts/records/merge-queue-state.schema.json",
        "contracts/records/integration-record.schema.json",
        "contracts/records/mainline-receipt-record.schema.json",
        "contracts/records/merge-record.schema.json",
        "contracts/records/promotion-record.schema.json",
        "contracts/records/regression-report.schema.json",
        "contracts/records/freeze-record.schema.json",
        "contracts/records/record-graph.contract.json",
    ];
    let declared_records = project
        .pointer("/governance/record_contracts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/governance/record_contracts"));
    if declared_records.len() != canonical_records.len()
        || canonical_records.iter().any(|path| {
            !declared_records
                .iter()
                .any(|value| value.as_str() == Some(*path))
        })
    {
        fail("NON_CANONICAL_RECORD_CONTRACT_SET");
    }
    let zone_text = match fs::read_to_string(&zone) {
        Ok(text) => text,
        Err(_) if !strict => return,
        Err(_) => fail("DECLARED_ZONE_CONTRACT_MISSING"),
    };
    let zone_value: Value =
        serde_json::from_str(&zone_text).unwrap_or_else(|_| fail("INVALID_DECLARED_ZONE_CONTRACT"));
    if zone_value.pointer("/zones")
        != Some(&serde_json::json!([
            "playground",
            "active",
            "protected",
            "generated"
        ]))
        || zone_value
            .pointer("/transitions")
            .and_then(Value::as_array)
            .map(|v| v.len())
            .unwrap_or(0)
            < 16
    {
        fail("INVALID_DECLARED_ZONE_CONTRACT");
    }
    let canonical_path = zone.with_file_name("zone-transition.manifest.json");
    let canonical_path = if canonical_path == zone {
        zone.with_file_name("zone-transition.manifest.json")
    } else {
        canonical_path
    };
    let canonical_value: Value = serde_json::from_str(include_str!(
        "../../contracts/transitions/zone-transition.manifest.json"
    ))
    .unwrap_or_else(|_| fail("INVALID_CANONICAL_ZONE_CONTRACT"));
    if canonical_project && zone_value != canonical_value {
        fail("DECLARED_ZONE_CONTRACT_MISMATCH");
    }
    if strict && !canonical_path.exists() {
        fail("CANONICAL_ZONE_CONTRACT_MISSING");
    }
    for declared in project
        .pointer("/governance/record_contracts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/governance/record_contracts"))
    {
        let relative = declared
            .as_str()
            .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/governance/record_contracts"));
        let path = safe_owned_path(root, relative, "record_contract");
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(_) if !strict => return,
            Err(_) => fail("DECLARED_RECORD_CONTRACT_MISSING"),
        };
        let value: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| fail("INVALID_DECLARED_RECORD_CONTRACT"));
        let canonical_text = match relative {
            "contracts/records/worktree-record.schema.json" => {
                include_str!("../../contracts/records/worktree-record.schema.json")
            }
            "contracts/records/reproduction-record.schema.json" => {
                include_str!("../../contracts/records/reproduction-record.schema.json")
            }
            "contracts/records/evidence-record.schema.json" => {
                include_str!("../../contracts/records/evidence-record.schema.json")
            }
            "contracts/records/goal-clarification-record.schema.json" => {
                include_str!("../../contracts/records/goal-clarification-record.schema.json")
            }
            "contracts/records/fix-candidate-record.schema.json" => {
                include_str!("../../contracts/records/fix-candidate-record.schema.json")
            }
            "contracts/records/review-record.schema.json" => {
                include_str!("../../contracts/records/review-record.schema.json")
            }
            "contracts/records/effectiveness-record.schema.json" => {
                include_str!("../../contracts/records/effectiveness-record.schema.json")
            }
            "contracts/records/pre-review-validation-record.schema.json" => {
                include_str!("../../contracts/records/pre-review-validation-record.schema.json")
            }
            "contracts/records/collaboration-record.schema.json" => {
                include_str!("../../contracts/records/collaboration-record.schema.json")
            }
            "contracts/records/collaboration-index.schema.json" => {
                include_str!("../../contracts/records/collaboration-index.schema.json")
            }
            "contracts/records/merge-queue-record.schema.json" => {
                include_str!("../../contracts/records/merge-queue-record.schema.json")
            }
            "contracts/records/merge-queue-state.schema.json" => {
                include_str!("../../contracts/records/merge-queue-state.schema.json")
            }
            "contracts/records/integration-record.schema.json" => {
                include_str!("../../contracts/records/integration-record.schema.json")
            }
            "contracts/records/mainline-receipt-record.schema.json" => {
                include_str!("../../contracts/records/mainline-receipt-record.schema.json")
            }
            "contracts/records/merge-record.schema.json" => {
                include_str!("../../contracts/records/merge-record.schema.json")
            }
            "contracts/records/promotion-record.schema.json" => {
                include_str!("../../contracts/records/promotion-record.schema.json")
            }
            "contracts/records/regression-report.schema.json" => {
                include_str!("../../contracts/records/regression-report.schema.json")
            }
            "contracts/records/freeze-record.schema.json" => {
                include_str!("../../contracts/records/freeze-record.schema.json")
            }
            "contracts/records/record-graph.contract.json" => {
                include_str!("../../contracts/records/record-graph.contract.json")
            }
            _ => fail("NON_CANONICAL_RECORD_CONTRACT_SET"),
        };
        let canonical_value: Value = serde_json::from_str(canonical_text)
            .unwrap_or_else(|_| fail("INVALID_CANONICAL_RECORD_CONTRACT"));
        if value != canonical_value {
            fail("DECLARED_RECORD_CONTRACT_MISMATCH");
        }
        if value.get("$schema").and_then(Value::as_str).is_none()
            || value.get("type").and_then(Value::as_str).is_none()
        {
            fail("INVALID_DECLARED_RECORD_CONTRACT");
        }
    }
}

fn assert_governance_maps(root: &Path) {
    for (name, key) in [
        ("resource-map.json", "resources"),
        ("module-registry.json", "modules"),
        ("function-map.json", "functions"),
        ("mainline-call-map.json", "edges"),
        ("verification-map.json", "gates"),
    ] {
        let file = root.join(".appsdk/maps").join(name);
        let value: Value = serde_json::from_str(
            &fs::read_to_string(&file)
                .unwrap_or_else(|_| fail(format!("MISSING_GOVERNANCE_MAP:{}", name))),
        )
        .unwrap_or_else(|_| fail(format!("INVALID_GOVERNANCE_MAP:{}", name)));
        if value.get("schema_version").and_then(Value::as_u64) != Some(1)
            || value
                .get(key)
                .and_then(Value::as_array)
                .map(|items| items.is_empty())
                .unwrap_or(true)
        {
            fail(format!("INVALID_GOVERNANCE_MAP:{}", name));
        }
        if name == "mainline-call-map.json" {
            let source = include_str!("main.rs");
            for edge in record_array(&value, "/edges", name) {
                for field in [
                    "/chain_id",
                    "/owner",
                    "/caller",
                    "/callee",
                    "/path",
                    "/input_resource_id",
                    "/output_resource_id",
                    "/error_resource_id",
                ] {
                    if record_str(edge, field, name).is_empty() {
                        fail("UNBOUND_MAINLINE_EDGE");
                    }
                }
                for symbol in [
                    record_str(edge, "/caller", name),
                    record_str(edge, "/callee", name),
                ] {
                    if !source.contains(&format!("fn {}", symbol)) {
                        fail(format!("MAINLINE_SYMBOL_MISSING:{}", symbol));
                    }
                }
                let caller = record_str(edge, "/caller", name);
                let callee = record_str(edge, "/callee", name);
                let caller_start = source
                    .find(&format!("fn {}(", caller))
                    .unwrap_or_else(|| fail(format!("MAINLINE_CALLER_MISSING:{}", caller)));
                let caller_source = &source[caller_start..];
                let caller_end = caller_source.find("\nfn ").unwrap_or(caller_source.len());
                if !caller_source[..caller_end].contains(&format!("{}(", callee)) {
                    fail(format!(
                        "MAINLINE_EDGE_NOT_IMPLEMENTED:{}->{}",
                        caller, callee
                    ));
                }
            }
        }
    }
}

fn registry_path_matches(pattern: &str, path: &str) -> bool {
    pattern
        .strip_suffix("/**")
        .map(|prefix| path == prefix || path.starts_with(&format!("{}/", prefix)))
        .unwrap_or(pattern == path)
}

fn assert_sdk_source_registry(root: &Path) {
    assert_project_root_safe(root);
    let registry: Value = serde_json::from_str(
        &fs::read_to_string(root.join("contracts/maps/module-registry.json"))
            .unwrap_or_else(|_| fail("MISSING_SDK_MODULE_REGISTRY")),
    )
    .unwrap_or_else(|_| fail("INVALID_SDK_MODULE_REGISTRY"));
    let modules = record_array(&registry, "/modules", "module-registry.json");
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()
        .unwrap_or_else(|_| fail("SDK_SOURCE_REGISTRY_GIT_UNAVAILABLE"));
    if !output.status.success() {
        fail("SDK_SOURCE_REGISTRY_GIT_FAILED");
    }
    for bytes in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|v| !v.is_empty())
    {
        let path = std::str::from_utf8(bytes).unwrap_or_else(|_| fail("INVALID_SDK_SOURCE_PATH"));
        let mut owners = Vec::new();
        for module in modules {
            let module_id = record_str(module, "/module_id", "module-registry.json");
            if module.get("status").and_then(Value::as_str) != Some("active") {
                continue;
            }
            if record_array(module, "/owned_paths", module_id)
                .iter()
                .any(|pattern| {
                    pattern
                        .as_str()
                        .is_some_and(|pattern| registry_path_matches(pattern, path))
                })
            {
                owners.push(module_id);
            }
            if record_array(module, "/forbidden_paths", module_id)
                .iter()
                .any(|pattern| {
                    pattern
                        .as_str()
                        .is_some_and(|pattern| registry_path_matches(pattern, path))
                })
            {
                fail(format!("SDK_SOURCE_FORBIDDEN_PATH:{}:{}", module_id, path));
            }
        }
        if owners.len() != 1 {
            fail(format!(
                "SDK_SOURCE_OWNER_CARDINALITY:{}:{}",
                path,
                owners.join(",")
            ));
        }
    }
    println!("{}", r#"{"ok":true,"gate":"sdk_source_registry"}"#);
}

fn required_str<'a>(value: &'a Value, path: &str, error: &str) -> &'a str {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail(error))
}

fn assert_identifier(value: &str, error: &str) {
    if value.is_empty()
        || !value.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        || !value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        fail(error);
    }
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

fn sha256(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn assert_goal_confirmed(root: &Path) {
    assert_goal_contract(root, true);
}

fn read_goal(root: &Path) -> Value {
    let file = root.join(".appsdk/goal.json");
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:goal");
    }
    serde_json::from_str(
        &fs::read_to_string(file).unwrap_or_else(|_| fail("MISSING_GOAL_CLARIFICATION_RECORD")),
    )
    .unwrap_or_else(|_| fail("INVALID_GOAL_CLARIFICATION_RECORD"))
}

fn assert_goal_contract(root: &Path, require_confirmed: bool) {
    let goal = read_goal(root);
    for key in [
        "goal_id",
        "raw_request",
        "understood_objective",
        "created_at",
    ] {
        if goal
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            fail("INVALID_GOAL_CLARIFICATION_RECORD");
        }
    }
    for key in [
        "acceptance_criteria",
        "non_goals",
        "assumptions",
        "ambiguities",
        "questions",
    ] {
        if goal.get(key).and_then(Value::as_array).is_none() {
            fail("INVALID_GOAL_CLARIFICATION_RECORD");
        }
    }
    if goal["acceptance_criteria"].as_array().unwrap().is_empty()
        || goal["acceptance_criteria"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().map(|entry| entry.is_empty()).unwrap_or(true))
        || goal["non_goals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().is_none())
        || goal["assumptions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().is_none())
        || goal["ambiguities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().is_none())
    {
        fail("INVALID_GOAL_CLARIFICATION_RECORD");
    }
    for question in goal["questions"].as_array().unwrap() {
        if question
            .get("question_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
            || question
                .get("question")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
            || !matches!(
                question.get("status").and_then(Value::as_str),
                Some("open" | "answered" | "not_required")
            )
            || question
                .get("answer")
                .map(|answer| !(answer.is_null() || answer.as_str().is_some()))
                .unwrap_or(false)
        {
            fail("INVALID_GOAL_CLARIFICATION_RECORD");
        }
    }
    let status = goal.get("status").and_then(Value::as_str).unwrap_or("");
    if !matches!(
        status,
        "received" | "parsed" | "clarification_pending" | "confirmed" | "admitted" | "superseded"
    ) {
        fail("INVALID_GOAL_CLARIFICATION_RECORD");
    }
    let open_questions = goal["questions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|question| question.get("status").and_then(Value::as_str) == Some("open"));
    if require_confirmed && open_questions {
        fail("GOAL_HAS_OPEN_QUESTIONS");
    }
    if require_confirmed {
        if !matches!(status, "confirmed" | "admitted") {
            fail(format!("GOAL_NOT_CONFIRMED:{}", status));
        }
        if goal
            .get("confirmed_by")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
            || goal
                .get("confirmed_at")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
        {
            fail("GOAL_CONFIRMATION_MISSING");
        }
        if status == "admitted" && goal.get("scope").and_then(Value::as_object).is_none() {
            fail("ADMITTED_GOAL_SCOPE_MISSING");
        }
    }
}

fn assert_sdk_lock(root: &Path, project: &Value, require_pinned: bool) {
    let file = root.join(".appsdk").join("sdk.lock");
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_lock");
    }
    let lock: Value = serde_json::from_str(
        &fs::read_to_string(file).unwrap_or_else(|_| fail("MISSING_SDK_LOCK")),
    )
    .unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
    if lock.get("sdk").and_then(Value::as_str) != Some("appsdk")
        || lock.get("version").and_then(Value::as_str)
            != project.pointer("/sdk/version").and_then(Value::as_str)
        || lock.get("contract_schema") != project.get("schema_version")
    {
        fail("INVALID_SDK_LOCK");
    }
    for key in ["digest", "compiler_digest"] {
        let digest = lock.get(key).and_then(Value::as_str).unwrap_or("");
        let placeholder = digest == "sha256:replace-with-compiled-sdk-digest"
            || digest == "sha256:replace-with-compiler-digest";
        if require_pinned && placeholder {
            fail("SDK_LOCK_NOT_PINNED");
        }
        if (!placeholder && digest.len() != 71)
            || !digest.starts_with("sha256:")
            || (!placeholder && !digest[7..].chars().all(|c| c.is_ascii_hexdigit()))
        {
            fail("INVALID_SDK_LOCK_DIGEST");
        }
    }
    for key in ["bundle_digest", "bundle_manifest_digest"] {
        let digest = lock.get(key).and_then(Value::as_str).unwrap_or("");
        let placeholder = digest == "sha256:replace-with-sdk-bundle-digest"
            || digest == "sha256:replace-with-bundle-manifest-digest";
        if require_pinned && placeholder {
            fail("SDK_LOCK_NOT_PINNED");
        }
        if (!placeholder && digest.len() != 71)
            || !digest.starts_with("sha256:")
            || (!placeholder && !digest[7..].chars().all(|c| c.is_ascii_hexdigit()))
        {
            fail("INVALID_SDK_BUNDLE_DIGEST");
        }
    }
    if require_pinned {
        if lock.get("binary_ref").and_then(Value::as_str) != Some("project-sdk") {
            fail("INVALID_SDK_LOCK_BINARY_REF");
        }
        let running = std::env::current_exe().unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
        let actual =
            digest_bytes(&fs::read(running).unwrap_or_else(|_| fail("SDK_BINARY_MISSING")));
        if lock.get("digest").and_then(Value::as_str) != Some(actual.as_str())
            || lock.get("compiler_digest").and_then(Value::as_str) != Some(actual.as_str())
        {
            fail("SDK_BINARY_DIGEST_MISMATCH");
        }
        if lock.get("bundle_digest").and_then(Value::as_str) != Some(sdk_bundle_digest().as_str())
            || lock.get("bundle_manifest_digest").and_then(Value::as_str)
                != Some(digest_bytes(SDK_BUNDLE_MANIFEST.as_bytes()).as_str())
        {
            fail("SDK_BUNDLE_DIGEST_MISMATCH");
        }
    }
    let manifest_resources = sdk_bundle_manifest_resources();
    match lock.get("bundle_resources") {
        Some(declared) if declared == &manifest_resources => {}
        Some(_) => fail("SDK_LOCK_BUNDLE_RESOURCES_MISMATCH"),
        None if require_pinned => fail("SDK_LOCK_BUNDLE_RESOURCES_MISSING"),
        None => {}
    }
}

fn build_artifact(project: &Value) -> Value {
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let compiled = modules
        .iter()
        .map(|module| {
            let mut output = serde_json::Map::new();
            for key in [
                "module_id",
                "stage",
                "owned_paths",
                "source_owner",
                "active_artifact",
                "generated_outputs",
                "regression",
            ] {
                output.insert(
                    key.into(),
                    module
                        .get(key)
                        .cloned()
                        .unwrap_or_else(|| fail(format!("INVALID_MODULE_SURFACES:{}", key))),
                );
            }
            Value::Object(output)
        })
        .collect::<Vec<_>>();
    let mut artifact = serde_json::Map::new();
    artifact.insert("artifact_schema".into(), Value::from(1));
    artifact.insert(
        "project_id".into(),
        project
            .get("project_id")
            .cloned()
            .unwrap_or_else(|| fail("INVALID_PROJECT_ID")),
    );
    artifact.insert(
        "sdk".into(),
        project
            .get("sdk")
            .cloned()
            .unwrap_or_else(|| fail("INVALID_SDK_CONTRACT")),
    );
    artifact.insert("modules".into(), Value::Array(compiled));
    let unsigned = Value::Object(artifact.clone());
    artifact.insert(
        "artifact_hash".into(),
        Value::String(sha256(&canonical(&unsigned))),
    );
    Value::Object(artifact)
}

fn write_artifact(root: &Path, project: &Value) -> Value {
    let artifact = build_artifact(project);
    write_artifact_value(root, project, &artifact);
    artifact
}

fn generated_root(root: &Path, project: &Value) -> PathBuf {
    contract_root(root, project, "/governance/generated_root")
}

fn contract_root(root: &Path, project: &Value, path: &str) -> PathBuf {
    let value = required_str(project, path, "INVALID_GOVERNANCE_CONTRACT");
    let relative = value.trim_end_matches("/**").trim_end_matches('/');
    let candidate = Path::new(relative);
    if relative.is_empty()
        || candidate.is_absolute()
        || candidate.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        fail(format!("INVALID_GOVERNANCE_ROOT:{}", path));
    }
    let current = root.join(candidate);
    assert_no_symlink_components(root, &current, path);
    current
}

fn assert_no_symlink_components(root: &Path, path: &Path, label: &str) {
    if fs::symlink_metadata(root)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(format!("GOVERNANCE_PATH_SYMLINK:{}", label));
    }
    let relative = path
        .strip_prefix(root)
        .unwrap_or_else(|_| fail(format!("GOVERNANCE_PATH_ESCAPE:{}", label)));
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        if fs::symlink_metadata(&current)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(format!("GOVERNANCE_PATH_SYMLINK:{}", label));
        }
    }
}

fn safe_owned_path(root: &Path, relative: &str, label: &str) -> PathBuf {
    let trimmed = relative.trim_end_matches("/**").trim_end_matches('/');
    let path = Path::new(trimmed);
    if trimmed.is_empty() || path.is_absolute() {
        fail(format!("INVALID_OWNED_PATH:{}", label));
    }
    if trimmed != "."
        && path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        fail(format!("INVALID_OWNED_PATH:{}", label));
    }
    let full = root.join(path);
    assert_no_symlink_components(root, &full, label);
    full
}

fn assert_vcs_clean(root: &Path, project: &Value, module_id: &str) {
    let probe = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "rev-parse",
            "--show-toplevel",
        ])
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !probe.status.success() {
        fail("VCS_ADAPTER_UNAVAILABLE");
    }
    let git_root = PathBuf::from(String::from_utf8_lossy(&probe.stdout).trim());
    let project_root = root
        .canonicalize()
        .unwrap_or_else(|_| fail("PROJECT_ROOT_MISSING"));
    let canonical_git_root = git_root
        .canonicalize()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    // The project may live in a subdirectory of a larger repository (for example
    // a V4 subproject inside a monorepo). Cleanliness must be scoped to the
    // project-relative prefix so unrelated sibling changes never block freeze.
    let prefix_probe = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "rev-parse",
            "--show-prefix",
        ])
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !prefix_probe.status.success() {
        fail("VCS_ADAPTER_UNAVAILABLE");
    }
    let _prefix = String::from_utf8_lossy(&prefix_probe.stdout)
        .trim()
        .to_string();
    if !project_root.starts_with(&canonical_git_root) {
        fail("VCS_PROJECT_ROOT_MISMATCH");
    }
    if !project
        .get("modules")
        .and_then(Value::as_array)
        .map(|modules| {
            modules
                .iter()
                .any(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or(false)
    {
        fail("MODULE_NOT_FOUND");
    }
    let mut vcs_scope = Command::new("git");
    vcs_scope.args([
        "-C",
        root.to_str().unwrap_or("."),
        "status",
        "--porcelain",
        "--",
    ]);
    vcs_scope.arg(project_root.to_str().unwrap_or("."));
    let output = vcs_scope
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !output.status.success() {
        fail("VCS_ADAPTER_FAILED");
    }
    let dirty = String::from_utf8_lossy(&output.stdout);
    for line in dirty.lines() {
        let paths = line.get(3..).unwrap_or("").trim();
        for path in paths.split(" -> ") {
            if !path.starts_with(".appsdk/transactions/") {
                fail("GIT_SCOPE_NOT_CLEAN");
            }
        }
    }
}

fn assert_protected_not_ignored(root: &Path, archive: &Path) {
    let relative = archive
        .strip_prefix(root)
        .unwrap_or_else(|_| fail("GOVERNANCE_PATH_ESCAPE:protected_archive"));
    let status = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "check-ignore",
            "--no-index",
            "--quiet",
            "--",
        ])
        .arg(relative)
        .status()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    match status.code() {
        Some(1) => {}
        Some(0) => fail("PROTECTED_ARCHIVE_IGNORED"),
        _ => fail("VCS_ADAPTER_FAILED"),
    }
}

fn copy_tree(source: &Path, target: &Path) {
    if fs::symlink_metadata(source)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("PROTECTED_ARCHIVE_SYMLINK");
    }
    if source.is_dir() {
        fs::create_dir_all(target).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
        for entry in fs::read_dir(source).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED")) {
            let entry = entry.unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
            if entry
                .file_type()
                .unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"))
                .is_symlink()
            {
                fail("PROTECTED_ARCHIVE_SYMLINK");
            }
            copy_tree(&entry.path(), &target.join(entry.file_name()));
        }
    } else if source.is_file() {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
        }
        fs::copy(source, target).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
    }
}

fn staging_path(root: &Path, project: &Value, module_id: &str) -> PathBuf {
    generated_root(root, project)
        .join("active-publish")
        .join(format!("{}.{}", module_id, std::process::id()))
}

fn module_generated_dir(root: &Path, project: &Value, module_id: &str) -> PathBuf {
    generated_root(root, project)
        .join("modules")
        .join(module_id)
}

fn module_artifact_file(root: &Path, project: &Value, module_id: &str) -> PathBuf {
    module_generated_dir(root, project, module_id).join("module.compiled.json")
}

fn module_lib_root(root: &Path, project: &Value, module_id: &str) -> PathBuf {
    module_generated_dir(root, project, module_id).join("lib")
}

fn safe_module_artifact_path(
    root: &Path,
    project: &Value,
    module_id: &str,
    relative: &str,
) -> PathBuf {
    let lib_root = module_lib_root(root, project, module_id);
    let candidate = Path::new(relative);
    if relative.is_empty()
        || candidate.is_absolute()
        || candidate.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        fail(format!("INVALID_MODULE_ARTIFACT_PATH:{}", module_id));
    }
    let target = lib_root.join(candidate);
    if target.starts_with(&lib_root) {
        target
    } else {
        fail(format!("INVALID_MODULE_ARTIFACT_PATH:{}", module_id));
    }
}

fn file_sha256(path: &Path, label: &str) -> String {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(format!("ARTIFACT_PATH_SYMLINK:{}", label));
    }
    let bytes = fs::read(path).unwrap_or_else(|_| fail(format!("ARTIFACT_PATH_MISSING:{}", label)));
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn hash_tree(root: &Path, prefix: &Path, label: &str) -> String {
    if !root.exists() {
        fail(format!("HASH_TREE_MISSING:{}", label));
    }
    let mut files = Vec::new();
    collect_files(root, prefix, label, &mut files);
    files.sort();
    let mut hasher = Sha256::new();
    for (relative, hash) in files {
        hasher.update(relative.as_os_str().as_encoded_bytes());
        hasher.update([0u8]);
        hasher.update(hash.as_bytes());
        hasher.update([0u8]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn collect_files(root: &Path, prefix: &Path, label: &str, files: &mut Vec<(PathBuf, String)>) {
    let entries =
        fs::read_dir(root).unwrap_or_else(|_| fail(format!("HASH_TREE_READ_FAILED:{}", label)));
    let mut entries = entries.collect::<Vec<_>>();
    entries.sort_by_key(|entry| {
        entry
            .as_ref()
            .map(|entry| entry.file_name())
            .unwrap_or_default()
    });
    for entry in entries {
        let entry = entry.unwrap_or_else(|_| fail(format!("HASH_TREE_READ_FAILED:{}", label)));
        if entry
            .file_type()
            .unwrap_or_else(|_| fail(format!("HASH_TREE_READ_FAILED:{}", label)))
            .is_symlink()
        {
            fail(format!("HASH_TREE_SYMLINK:{}", label));
        }
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, prefix, label, files);
        } else if path.is_file() {
            let relative = path
                .strip_prefix(prefix)
                .unwrap_or_else(|_| fail(format!("HASH_TREE_PREFIX:{}", label)))
                .to_path_buf();
            files.push((relative, file_sha256(&path, label)));
        }
    }
}

fn module_build_command(module: &Value, module_id: &str) -> Value {
    module
        .get("build")
        .cloned()
        .unwrap_or_else(|| fail(format!("INVALID_MODULE_BUILD_CONTRACT:{}", module_id)))
}

fn run_module_build(root: &Path, module: &Value, module_id: &str) {
    let build = module_build_command(module, module_id);
    let program = build
        .get("program")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail(format!("INVALID_MODULE_BUILD_CONTRACT:{}", module_id)));
    let args = build
        .get("args")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            fail(format!("INVALID_MODULE_BUILD_CONTRACT:{}", module_id))
                        })
                        .to_string()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| fail(format!("INVALID_MODULE_BUILD_CONTRACT:{}", module_id)));
    let working_directory = build
        .get("working_directory")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail(format!("INVALID_MODULE_BUILD_CONTRACT:{}", module_id)));
    let working = safe_owned_path(root, working_directory, "module_build_working_directory");
    let remap_root = root
        .canonicalize()
        .unwrap_or_else(|_| fail(format!("MODULE_BUILD_FAILED:{}", module_id)));
    let remap_flag = format!("--remap-path-prefix={}={}", remap_root.display(), ".");
    let mut command = Command::new(program);
    command.args(&args).current_dir(&working);
    let rustflags = match std::env::var("RUSTFLAGS") {
        Ok(existing) if !existing.trim().is_empty() => format!("{} {}", existing, remap_flag),
        _ => remap_flag,
    };
    command.env("RUSTFLAGS", rustflags);
    let output = command
        .output()
        .unwrap_or_else(|_| fail(format!("MODULE_BUILD_FAILED:{}", module_id)));
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        fail(format!("MODULE_BUILD_FAILED:{}", module_id));
    }
}

fn hash_module_paths(
    root: &Path,
    _project: &Value,
    module: &Value,
    module_id: &str,
    key: &str,
) -> String {
    let paths = module
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail(format!("INVALID_MODULE_CONTRACT:{}:{}", module_id, key)));
    let mut hasher = Sha256::new();
    for path in paths {
        let relative = path
            .as_str()
            .unwrap_or_else(|| fail(format!("INVALID_MODULE_CONTRACT:{}:{}", module_id, key)));
        let safe = safe_owned_path(root, relative, "module_path_hash");
        let mut base = safe.clone();
        if relative.ends_with("/**") {
            base = safe_owned_path(
                root,
                relative.trim_end_matches("/**").trim_end_matches('/'),
                "module_path_hash",
            );
        }
        hasher.update(relative.as_bytes());
        hasher.update([0u8]);
        if safe.is_file() {
            hasher.update(file_sha256(&safe, "module_path").as_bytes());
        } else if safe.is_dir() || (relative.ends_with("/**") && base.exists()) {
            hasher.update(hash_tree(&base, &base, relative).as_bytes());
        } else {
            fail(format!("MODULE_PATH_MISSING:{}:{}", module_id, relative));
        }
        hasher.update([0u8]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn module_dependency_hashes(
    root: &Path,
    project: &Value,
    module: &Value,
    module_id: &str,
) -> Vec<Value> {
    let dependencies = module
        .get("dependency_modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            fail(format!(
                "INVALID_MODULE_CONTRACT:{}:dependency_modules",
                module_id
            ))
        });
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let mut entries = Vec::new();
    for dependency in dependencies {
        let dependency_id = dependency
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                fail(format!(
                    "INVALID_MODULE_CONTRACT:{}:dependency_modules",
                    module_id
                ))
            });
        let dependency_module = modules
            .iter()
            .find(|module| module.get("module_id").and_then(Value::as_str) == Some(dependency_id))
            .unwrap_or_else(|| {
                fail(format!(
                    "MODULE_DEPENDENCY_NOT_FOUND:{}:{}",
                    module_id, dependency_id
                ))
            });
        if dependency_module.get("stage").and_then(Value::as_str) != Some("frozen") {
            fail(format!(
                "MODULE_DEPENDENCY_NOT_FROZEN:{}:{}",
                module_id, dependency_id
            ));
        }
        let artifact_file = module_artifact_file(root, project, dependency_id);
        if !artifact_file.is_file() {
            fail(format!(
                "MODULE_DEPENDENCY_ARTIFACT_MISSING:{}",
                dependency_id
            ));
        }
        let artifact: Value = serde_json::from_str(
            &fs::read_to_string(&artifact_file)
                .unwrap_or_else(|_| fail("MODULE_ARTIFACT_READ_FAILED")),
        )
        .unwrap_or_else(|_| fail("INVALID_MODULE_ARTIFACT"));
        let hash = record_str(&artifact, "/artifact_hash", "module-artifact");
        entries.push(serde_json::json!({"module_id": dependency_id, "artifact_hash": hash}));
    }
    entries
}

fn hash_module_artifacts(
    root: &Path,
    project: &Value,
    module: &Value,
    module_id: &str,
) -> Vec<Value> {
    let paths = module
        .get("artifact_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            fail(format!(
                "INVALID_MODULE_CONTRACT:{}:artifact_paths",
                module_id
            ))
        });
    let mut entries = Vec::new();
    for path in paths {
        let relative = path
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                fail(format!(
                    "INVALID_MODULE_CONTRACT:{}:artifact_paths",
                    module_id
                ))
            });
        let target = safe_module_artifact_path(root, project, module_id, relative);
        entries.push(serde_json::json!({
            "path": relative,
            "hash": file_sha256(&target, &format!("module_artifact:{}", module_id))
        }));
    }
    entries
}

fn module_public_api_hash(artifact_entries: &[Value]) -> String {
    let mut hasher = Sha256::new();
    for entry in artifact_entries {
        hasher.update(
            entry
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .as_bytes(),
        );
        hasher.update([0u8]);
        hasher.update(
            entry
                .get("hash")
                .and_then(Value::as_str)
                .unwrap_or("")
                .as_bytes(),
        );
        hasher.update([0u8]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn build_module_artifact(root: &Path, project: &Value, module: &Value, module_id: &str) -> Value {
    let source_hash = hash_module_paths(root, project, module, module_id, "owned_paths");
    let contract_hash = hash_module_paths(root, project, module, module_id, "contract_paths");
    let dependency_hashes = module_dependency_hashes(root, project, module, module_id);
    let build_command = module_build_command(module, module_id);
    let artifact_entries = hash_module_artifacts(root, project, module, module_id);
    let public_api_hash = module_public_api_hash(&artifact_entries);
    let mut unsigned = serde_json::Map::new();
    unsigned.insert("artifact_schema".into(), Value::from(1));
    unsigned.insert("module_id".into(), Value::String(module_id.into()));
    unsigned.insert(
        "stage".into(),
        module
            .get("stage")
            .cloned()
            .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT")),
    );
    unsigned.insert("source_hash".into(), Value::String(source_hash));
    unsigned.insert("contract_hash".into(), Value::String(contract_hash));
    unsigned.insert("dependency_hashes".into(), Value::Array(dependency_hashes));
    unsigned.insert("build".into(), build_command);
    unsigned.insert(
        "artifact_paths".into(),
        module
            .get("artifact_paths")
            .cloned()
            .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT")),
    );
    unsigned.insert("artifacts".into(), Value::Array(artifact_entries));
    unsigned.insert("public_api_hash".into(), Value::String(public_api_hash));
    let mut unsigned = unsigned;
    unsigned.remove("stage");
    let unsigned_value = Value::Object(unsigned);
    let artifact_hash = sha256(&canonical(&unsigned_value));
    let mut artifact = unsigned_value
        .as_object()
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
        .clone();
    artifact.insert(
        "stage".into(),
        module
            .get("stage")
            .cloned()
            .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT")),
    );
    artifact.insert("artifact_hash".into(), Value::String(artifact_hash));
    Value::Object(artifact)
}

fn read_module_artifact(root: &Path, project: &Value, module_id: &str) -> Value {
    let file = module_artifact_file(root, project, module_id);
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:module_artifact");
    }
    serde_json::from_str(
        &fs::read_to_string(&file).unwrap_or_else(|_| fail("MISSING_RECORD:module-artifact")),
    )
    .unwrap_or_else(|_| fail("INVALID_MODULE_ARTIFACT"))
}

fn write_module_artifact_value(root: &Path, project: &Value, module_id: &str, artifact: &Value) {
    let dir = module_generated_dir(root, project, module_id);
    fs::create_dir_all(&dir).unwrap_or_else(|_| fail("MODULE_ARTIFACT_WRITE_FAILED"));
    let target = dir.join("module.compiled.json");
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:module_artifact");
    }
    atomic_write_json(&target, artifact, "MODULE_ARTIFACT_WRITE_FAILED");
}

fn module_artifact_matches_project(module: &Value, artifact: &Value) -> Value {
    let module_id = record_str(module, "/module_id", "module");
    if artifact.get("artifact_schema").and_then(Value::as_u64) != Some(1)
        || record_str(artifact, "/module_id", "module-artifact") != module_id
        || artifact.get("build") != module.get("build")
        || artifact.get("artifact_paths") != module.get("artifact_paths")
    {
        fail(format!("MODULE_ARTIFACT_MISMATCH:{}", module_id));
    }
    let stored_hash = record_str(artifact, "/artifact_hash", "module-artifact");
    let mut unsigned = artifact.clone();
    unsigned
        .as_object_mut()
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
        .remove("artifact_hash");
    unsigned
        .as_object_mut()
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
        .remove("stage");
    if stored_hash != sha256(&canonical(&unsigned)) {
        fail(format!("MODULE_ARTIFACT_HASH_MISMATCH:{}", module_id));
    }
    artifact.clone()
}

/// Previous-active contract check: the already-published Active surface must
/// keep the same module identity and artifact surface, and must stay
/// self-consistent (its own signed hash still recomputes). The `build`
/// command is per-version reproduction metadata and may legitimately change
/// when a new version is opened (for example migrating a frozen consumer to a
/// resolver-managed link surface); it is therefore not compared against the
/// current module contract. The previous artifact remains hash-bound by its
/// own freeze record and by `version_base.base_artifact_hash`.
fn previous_active_matches_module(module: &Value, artifact: &Value) {
    let module_id = record_str(module, "/module_id", "module");
    if artifact.get("artifact_schema").and_then(Value::as_u64) != Some(1)
        || record_str(artifact, "/module_id", "module-artifact") != module_id
        || artifact.get("artifact_paths") != module.get("artifact_paths")
    {
        fail(format!("MODULE_ARTIFACT_MISMATCH:{}", module_id));
    }
    let stored_hash = record_str(artifact, "/artifact_hash", "module-artifact");
    let mut unsigned = artifact.clone();
    unsigned
        .as_object_mut()
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
        .remove("artifact_hash");
    unsigned
        .as_object_mut()
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
        .remove("stage");
    if stored_hash != sha256(&canonical(&unsigned)) {
        fail(format!("MODULE_ARTIFACT_HASH_MISMATCH:{}", module_id));
    }
}

fn compile_module(root: &Path, module_id: &str) -> Value {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_declared_contracts(root, &project, true);
    assert_goal_confirmed(root);
    assert_project_contract(root, &project);
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let index = modules
        .iter()
        .position(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    let module = &modules[index];
    if module.get("stage").and_then(Value::as_str) == Some("frozen") {
        fail(format!(
            "FROZEN_MODULE_REQUIRES_VERSIONED_ARTIFACT:{}",
            module_id
        ));
    }
    run_module_build(root, module, module_id);
    let mut module_with_stage = module.clone();
    let target_stage = module
        .get("stage")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT"))
        .to_string();
    module_with_stage["stage"] = Value::String(target_stage);
    let artifact = build_module_artifact(root, &project, &module_with_stage, module_id);
    write_module_artifact_value(root, &project, module_id, &artifact);
    println!("{}", serde_json::to_string_pretty(&artifact).unwrap());
    artifact
}

fn write_artifact_value(root: &Path, project: &Value, artifact: &Value) {
    let dir = generated_root(root, project);
    fs::create_dir_all(&dir).unwrap_or_else(|_| fail("ARTIFACT_WRITE_FAILED"));
    let target = dir.join("project.compiled.json");
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:artifact");
    }
    atomic_write_json(&target, artifact, "ARTIFACT_WRITE_FAILED");
}

fn read_compiled_artifact(root: &Path, project: &Value) -> Value {
    let file = generated_root(root, project).join("project.compiled.json");
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:artifact");
    }
    serde_json::from_str(
        &fs::read_to_string(&file).unwrap_or_else(|_| fail("INVALID_ARTIFACT_SCHEMA")),
    )
    .unwrap_or_else(|_| fail("INVALID_ARTIFACT_SCHEMA"))
}

fn assert_artifact_matches(project: &Value, artifact: &Value) {
    if artifact.get("artifact_schema").and_then(Value::as_u64) != Some(1)
        || artifact.get("project_id").and_then(Value::as_str)
            != project.get("project_id").and_then(Value::as_str)
        || artifact.pointer("/sdk/name").and_then(Value::as_str) != Some("appsdk")
        || artifact.pointer("/sdk/version") != project.pointer("/sdk/version")
        || artifact.get("modules").and_then(Value::as_array).is_none()
    {
        fail("INVALID_ARTIFACT_SCHEMA");
    }
    let stored_hash = record_str(artifact, "/artifact_hash", "artifact");
    let mut unsigned = artifact.clone();
    unsigned
        .as_object_mut()
        .unwrap_or_else(|| fail("INVALID_ARTIFACT_SCHEMA"))
        .remove("artifact_hash");
    if stored_hash != sha256(&canonical(&unsigned)) {
        fail("ARTIFACT_HASH_MISMATCH");
    }
    let project_modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let artifact_modules = artifact
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_ARTIFACT_SCHEMA"));
    if project_modules.len() != artifact_modules.len() {
        fail("ARTIFACT_MODULE_SET_MISMATCH");
    }
    let mut artifact_ids = std::collections::HashSet::new();
    for entry in artifact_modules {
        let id = record_str(entry, "/module_id", "artifact");
        if !artifact_ids.insert(id) {
            fail(format!("DUPLICATE_MODULE:{}", id));
        }
        let stage = record_str(entry, "/stage", "artifact");
        if !matches!(
            stage,
            "draft"
                | "source_implemented"
                | "contract_bound"
                | "compiled"
                | "controlled_verified"
                | "architecture_stable"
                | "frozen"
                | "retired"
        ) {
            fail(format!("INVALID_MODULE_CONTRACT:{}", id));
        }
    }
    for module in project_modules {
        let module_id = record_str(module, "/module_id", "module");
        let compiled = artifact_modules
            .iter()
            .find(|entry| entry.get("module_id").and_then(Value::as_str) == Some(module_id))
            .unwrap_or_else(|| fail(format!("ARTIFACT_MODULE_MISMATCH:{}", module_id)));
        for key in [
            "stage",
            "source_owner",
            "active_artifact",
            "owned_paths",
            "generated_outputs",
            "regression",
        ] {
            if key == "stage"
                && module.get("version_base").is_some()
                && module.get("stage").and_then(Value::as_str) == Some("source_implemented")
                && compiled.get("stage").and_then(Value::as_str) == Some("frozen")
            {
                continue;
            }
            if key == "stage"
                && module.get("stage").and_then(Value::as_str) == Some("frozen")
                && compiled.get("stage").and_then(Value::as_str) == Some("architecture_stable")
            {
                continue;
            }
            if compiled.get(key) != module.get(key) {
                fail(format!("ARTIFACT_MODULE_MISMATCH:{}", module_id));
            }
        }
    }
}

fn assert_compile_preconditions(root: &Path, project: &Value, changing_module: Option<&str>) {
    assert_project_contract(root, project);
    assert_goal_confirmed(root);
    assert_sdk_lock(root, project, true);
    let stage = required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT");
    if !matches!(
        stage,
        "contract_bound" | "compiled" | "controlled_verified" | "architecture_stable"
    ) {
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "error": "COMPILE_BLOCKED",
                "current_stage": stage,
                "required_stage": "contract_bound",
                "retry_allowed": false,
                "idempotent": true,
                "next": [
                    "confirm .appsdk/goal.json through the user-approved goal clarification flow",
                    "appsdk promote <project> --to source_implemented",
                    "appsdk promote <project> --to contract_bound",
                    "rerun appsdk compile <project> once the project is contract_bound"
                ],
                "forbidden": [
                    "do not create generated/module artifacts by hand",
                    "do not edit lifecycle stage directly",
                    "do not retry compile before the stage changes"
                ]
            }))
            .unwrap()
        );
        std::process::exit(1);
    }
    if changing_module.is_none()
        && project
            .get("modules")
            .and_then(Value::as_array)
            .map(|modules| {
                modules.iter().all(|module| {
                    matches!(
                        module.get("stage").and_then(Value::as_str),
                        Some("frozen" | "retired")
                    )
                })
            })
            .unwrap_or(false)
    {
        fail("FROZEN_ARTIFACT_IMMUTABLE");
    }
}

fn assert_development_scenarios(root: &Path, project: &Value) -> bool {
    let manifest_path = project
        .pointer("/development_scenarios/manifest")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/development_scenarios/manifest"));
    if manifest_path != ".appsdk/contracts/development-scenarios.manifest.json" {
        fail("NON_CANONICAL_DEVELOPMENT_SCENARIO_MANIFEST");
    }
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(safe_owned_path(
            root,
            manifest_path,
            "development_scenarios",
        ))
        .unwrap_or_else(|_| fail("DEVELOPMENT_SCENARIO_MANIFEST_MISSING")),
    )
    .unwrap_or_else(|_| fail("INVALID_DEVELOPMENT_SCENARIO_MANIFEST"));
    let canonical_manifest: Value = serde_json::from_str(include_str!(
        "../../contracts/development-scenarios.manifest.json"
    ))
    .unwrap();
    if manifest != canonical_manifest {
        fail("DEVELOPMENT_SCENARIO_MANIFEST_MISMATCH");
    }
    let enabled = project
        .pointer("/development_scenarios/enabled")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/development_scenarios/enabled"));
    let mut multi_worker = false;
    let mut multi_worktree = false;
    for scenario in enabled {
        match scenario.as_str() {
            Some("multi_worker_collaboration") if !multi_worker => multi_worker = true,
            Some("multi_worktree_merge_queue") if !multi_worktree => multi_worktree = true,
            Some("multi_worker_collaboration" | "multi_worktree_merge_queue") => {
                fail("DUPLICATE_DEVELOPMENT_SCENARIO")
            }
            _ => fail("UNKNOWN_DEVELOPMENT_SCENARIO"),
        }
    }
    if multi_worker != multi_worktree {
        fail("DEVELOPMENT_SCENARIO_PAIR_REQUIRED");
    }
    multi_worker
}

fn assert_project_contract(root: &Path, project: &Value) {
    if project.get("schema_version").and_then(Value::as_u64) != Some(1)
        || project.get("project_id").and_then(Value::as_str).is_none()
        || project.pointer("/sdk/name").and_then(Value::as_str) != Some("appsdk")
        || project
            .pointer("/sdk/version")
            .and_then(Value::as_str)
            .is_none()
        || project
            .pointer("/lifecycle/stage")
            .and_then(Value::as_str)
            .is_none()
        || project
            .pointer("/access/protected_paths")
            .and_then(Value::as_array)
            .is_none()
    {
        fail("INVALID_PROJECT_CONTRACT");
    }
    let sdk_version = project
        .pointer("/sdk/version")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/sdk/version"));
    if sdk_version != "0.1.6" {
        fail(format!(
            "PROJECT_SDK_VERSION_PIN_MISMATCH:{}:required_binary=appsdk-{}",
            sdk_version, sdk_version
        ));
    }
    let _ = assert_development_scenarios(root, project);
    assert_identifier(
        project
            .get("project_id")
            .and_then(Value::as_str)
            .unwrap_or(""),
        "INVALID_PROJECT_ID",
    );
    if project
        .pointer("/access/protected_paths")
        .and_then(Value::as_array)
        .map(|values| {
            values.is_empty()
                || values
                    .iter()
                    .any(|value| value.as_str().filter(|v| !v.is_empty()).is_none())
        })
        .unwrap_or(true)
    {
        fail("INVALID_PROJECT_CONTRACT:/access/protected_paths");
    }
    for path in [
        "/governance/playground_root",
        "/governance/active_root",
        "/governance/protected_root",
        "/governance/generated_root",
        "/governance/active_kind",
        "/governance/zone_transition_contract",
        "/governance/playground_retention",
    ] {
        if project.pointer(path).and_then(Value::as_str).is_none() {
            fail(format!("INVALID_PROJECT_CONTRACT:{}", path));
        }
    }
    for path in [
        "/governance/protected_kinds",
        "/governance/generated_kinds",
        "/governance/freeze_requirements",
        "/governance/promotion_requires",
        "/governance/runtime_forbidden_roots",
        "/governance/record_contracts",
    ] {
        if project.pointer(path).and_then(Value::as_array).is_none()
            || project
                .pointer(path)
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .any(|value| value.as_str().filter(|v| !v.is_empty()).is_none())
                })
                .unwrap_or(true)
        {
            fail(format!("INVALID_PROJECT_CONTRACT:{}", path));
        }
    }
    if project
        .pointer("/governance/active_kind")
        .and_then(Value::as_str)
        != Some("immutable_consumable_library")
    {
        fail("INVALID_PROJECT_CONTRACT:/governance/active_kind");
    }
    if project.pointer("/governance/debug_merge_comment_required") != Some(&Value::Bool(true))
        || !matches!(
            project
                .pointer("/governance/playground_retention")
                .and_then(Value::as_str),
            Some("archive_then_remove" | "archive_only")
        )
    {
        fail("INVALID_PROJECT_CONTRACT:/governance/lifecycle_controls");
    }
    let roots = [
        project
            .pointer("/governance/playground_root")
            .and_then(Value::as_str)
            .unwrap(),
        project
            .pointer("/governance/active_root")
            .and_then(Value::as_str)
            .unwrap(),
        project
            .pointer("/governance/protected_root")
            .and_then(Value::as_str)
            .unwrap(),
        project
            .pointer("/governance/generated_root")
            .and_then(Value::as_str)
            .unwrap(),
    ];
    for (index, left) in roots.iter().enumerate() {
        let left = left.trim_end_matches("/**").trim_end_matches('/');
        let left_path = Path::new(left);
        if left.is_empty()
            || left_path.is_absolute()
            || left_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            fail("INVALID_GOVERNANCE_ROOT");
        }
        for right in roots.iter().skip(index + 1) {
            let right = right.trim_end_matches("/**").trim_end_matches('/');
            if left == right
                || left.starts_with(&format!("{}/", right))
                || right.starts_with(&format!("{}/", left))
            {
                fail("OVERLAPPING_GOVERNANCE_ROOTS");
            }
        }
    }
    for path in [
        "/lifecycles/issue",
        "/lifecycles/library",
        "/lifecycles/source_snapshot",
        "/lifecycles/artifact",
    ] {
        if project.pointer(path).and_then(Value::as_str).is_none() {
            fail(format!("INVALID_PROJECT_CONTRACT:{}", path));
        }
    }
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/modules"));
    let mut ids = std::collections::HashSet::new();
    let mut owned_surfaces: Vec<(String, String)> = Vec::new();
    for module in modules {
        let id = module
            .get("module_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        assert_identifier(id, "INVALID_PROJECT_MODULE");
        if !ids.insert(id)
            || !matches!(
                module.get("stage").and_then(Value::as_str),
                Some(
                    "draft"
                        | "source_implemented"
                        | "contract_bound"
                        | "compiled"
                        | "controlled_verified"
                        | "architecture_stable"
                        | "frozen"
                        | "retired"
                )
            )
            || module
                .get("source_owner")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .is_none()
            || module.get("source_owner").and_then(Value::as_str) != Some(id)
            || module
                .get("active_artifact")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .is_none()
            || module
                .get("owned_paths")
                .and_then(Value::as_array)
                .map(|values| {
                    values.is_empty()
                        || values
                            .iter()
                            .any(|value| value.as_str().filter(|v| !v.is_empty()).is_none())
                })
                .unwrap_or(true)
            || module
                .get("generated_outputs")
                .and_then(Value::as_array)
                .map(|values| {
                    values.is_empty()
                        || values
                            .iter()
                            .any(|value| value.as_str().filter(|v| !v.is_empty()).is_none())
                })
                .unwrap_or(true)
            || module
                .get("contract_paths")
                .and_then(Value::as_array)
                .map(|values| {
                    values.is_empty()
                        || values
                            .iter()
                            .any(|value| value.as_str().filter(|v| !v.is_empty()).is_none())
                })
                .unwrap_or(true)
            || module
                .get("dependency_modules")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .any(|value| value.as_str().filter(|v| !v.is_empty()).is_none())
                })
                .unwrap_or(true)
            || module.get("build").and_then(Value::as_object).is_none()
            || module
                .get("artifact_paths")
                .and_then(Value::as_array)
                .map(|values| {
                    values.is_empty()
                        || values
                            .iter()
                            .any(|value| value.as_str().filter(|v| !v.is_empty()).is_none())
                })
                .unwrap_or(true)
        {
            fail(format!("INVALID_PROJECT_MODULE:{}", id));
        }
        if let Some(version_base) = module.get("version_base") {
            for path in [
                "/previous_active_version",
                "/new_active_version",
                "/base_artifact_hash",
                "/base_source_commit",
            ] {
                record_str(version_base, path, "module-version-base");
            }
            if version_base
                .get("previous_active_version")
                .and_then(Value::as_str)
                == version_base
                    .get("new_active_version")
                    .and_then(Value::as_str)
            {
                fail(format!("INVALID_MODULE_VERSION_BASE:{}", id));
            }
        }
        let build = module
            .get("build")
            .and_then(Value::as_object)
            .unwrap_or_else(|| fail(format!("INVALID_PROJECT_MODULE:{}", id)));
        for key in ["program", "working_directory"] {
            if build
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                fail(format!("INVALID_PROJECT_MODULE:{}:build/{}", id, key));
            }
        }
        if build
            .get("args")
            .and_then(Value::as_array)
            .map(|values| values.iter().any(|value| value.as_str().is_none()))
            .unwrap_or(true)
        {
            fail(format!("INVALID_PROJECT_MODULE:{}:build/args", id));
        }
        for dependency in module
            .get("dependency_modules")
            .and_then(Value::as_array)
            .unwrap_or_else(|| fail(format!("INVALID_PROJECT_MODULE:{}", id)))
        {
            let dependency = dependency
                .as_str()
                .unwrap_or_else(|| fail(format!("INVALID_PROJECT_MODULE:{}", id)));
            if !ids.contains(dependency) && dependency != id {
                fail(format!("INVALID_PROJECT_MODULE:{}:dependency", id));
            }
        }
        for value in module["owned_paths"]
            .as_array()
            .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"))
        {
            let path = value
                .as_str()
                .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"))
                .trim_end_matches("/**")
                .trim_end_matches('/')
                .to_string();
            owned_surfaces.push((path, id.to_string()));
            safe_owned_path(
                root,
                value
                    .as_str()
                    .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE")),
                "module_owned_path",
            );
        }
        safe_owned_path(
            root,
            module["active_artifact"]
                .as_str()
                .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE")),
            "module_active_artifact",
        );
        owned_surfaces.push((
            module["active_artifact"]
                .as_str()
                .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"))
                .trim_end_matches("/**")
                .trim_end_matches('/')
                .to_string(),
            id.to_string(),
        ));
        for value in module["generated_outputs"]
            .as_array()
            .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"))
        {
            safe_owned_path(
                root,
                value
                    .as_str()
                    .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE")),
                "module_generated_output",
            );
        }
        for value in module["contract_paths"]
            .as_array()
            .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"))
        {
            safe_owned_path(
                root,
                value
                    .as_str()
                    .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE")),
                "module_contract_path",
            );
        }
    }
    for (index, (left, left_owner)) in owned_surfaces.iter().enumerate() {
        for (right, right_owner) in owned_surfaces.iter().skip(index + 1) {
            if left_owner != right_owner
                && (left == right
                    || left.starts_with(&format!("{}/", right))
                    || right.starts_with(&format!("{}/", left)))
            {
                fail("OVERLAPPING_MODULE_OWNERSHIP");
            }
        }
    }
}

fn compile(root: &Path) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    let project = read_project(root);
    assert_compile_preconditions(root, &project, None);
    assert_declared_contracts(root, &project, true);
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    for module in modules {
        let module_id = record_str(module, "/module_id", "module");
        if module.get("stage").and_then(Value::as_str) != Some("frozen") {
            compile_module(root, module_id);
        }
    }
    let artifact = write_artifact(root, &project);
    println!("{}", serde_json::to_string_pretty(&artifact).unwrap());
}

fn write_project(root: &Path, project: &Value) {
    assert_no_symlink_components(root, &root.join(".appsdk"), "appsdk_control");
    let target = project_file(root);
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:project");
    }
    atomic_write_json(&target, project, "PROJECT_WRITE_FAILED");
}

fn begin_version(root: &Path, module_id: &str, from: &str, to: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    assert_version(from, "INVALID_ACTIVE_VERSION");
    assert_version(to, "INVALID_ACTIVE_VERSION");
    if from == to {
        fail("MODULE_VERSION_MUST_ADVANCE");
    }
    let project = read_project(root);
    assert_declared_contracts(root, &project, true);
    assert_goal_confirmed(root);
    assert_project_contract(root, &project);
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let index = modules
        .iter()
        .position(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    if modules[index].get("stage").and_then(Value::as_str) != Some("frozen") {
        fail(format!("MODULE_VERSION_REQUIRES_FROZEN:{}", module_id));
    }

    let active_root = contract_root(root, &project, "/governance/active_root");
    let module_active = active_root.join(module_id);
    let current_file = module_active.join("current.json");
    let from_path = module_active.join(from);
    let to_path = module_active.join(to);
    assert_no_symlink_components(root, &module_active, "active_module");
    assert_no_symlink_components(root, &current_file, "active_index");
    assert_no_symlink_components(root, &from_path, "previous_active");
    assert_no_symlink_components(root, &to_path, "new_active");
    let current: Value = serde_json::from_str(
        &fs::read_to_string(&current_file).unwrap_or_else(|_| fail("ACTIVE_INDEX_MISSING")),
    )
    .unwrap_or_else(|_| fail("INVALID_ACTIVE_INDEX"));
    if current.get("module_id").and_then(Value::as_str) != Some(module_id)
        || current.get("version").and_then(Value::as_str) != Some(from)
    {
        fail("MODULE_VERSION_FROM_NOT_CURRENT");
    }
    if !from_path.is_dir() {
        fail("PREVIOUS_ACTIVE_MISSING");
    }
    if to_path.exists() {
        fail(format!("ACTIVE_VERSION_EXISTS:{}", to));
    }
    let previous_artifact_file = from_path.join("artifact.json");
    let previous_artifact: Value = serde_json::from_str(
        &fs::read_to_string(&previous_artifact_file)
            .unwrap_or_else(|_| fail("PREVIOUS_ACTIVE_ARTIFACT_MISSING")),
    )
    .unwrap_or_else(|_| fail("INVALID_PREVIOUS_ACTIVE_ARTIFACT"));
    previous_active_matches_module(&modules[index], &previous_artifact);
    let previous_hash = record_str(
        &previous_artifact,
        "/artifact_hash",
        "previous_active_artifact",
    );
    if current.get("artifact_hash").and_then(Value::as_str) != Some(previous_hash) {
        fail("ACTIVE_INDEX_MISMATCH");
    }
    let freeze = read_record(root, &freeze_record_name(module_id));
    if record_str(&freeze, "/active_version", "freeze-record.json") != from
        || record_str(&freeze, "/library_hash", "freeze-record.json") != previous_hash
    {
        fail("MODULE_VERSION_FREEZE_MISMATCH");
    }
    let protected_archive = contract_root(root, &project, "/governance/protected_root")
        .join("history")
        .join(module_id);
    assert_no_symlink_components(root, &protected_archive, "protected_archive");
    if !protected_archive.is_dir() {
        fail("PROTECTED_HISTORY_MISSING");
    }

    let history_root = root.join(".appsdk").join("records").join("history");
    let history_version = history_root.join(module_id).join(from);
    assert_no_symlink_components(root, &history_root, "record_history");
    assert_no_symlink_components(root, &history_version, "record_history_version");
    if history_version.exists() {
        fail("MODULE_VERSION_HISTORY_EXISTS");
    }
    fs::create_dir_all(&history_version).unwrap_or_else(|_| fail("MODULE_VERSION_OPEN_FAILED"));
    for name in [
        module_record_name("evidence-record", module_id),
        module_record_name("review-record", module_id),
        module_record_name("promotion-record", module_id),
        module_record_name("regression-report", module_id),
        freeze_record_name(module_id),
    ] {
        fs::copy(
            root.join(".appsdk").join("records").join(&name),
            history_version.join(&name),
        )
        .unwrap_or_else(|_| fail("MODULE_VERSION_HISTORY_INCOMPLETE"));
    }
    let promotion = read_record(root, &module_record_name("promotion-record", module_id));
    let cleanup_id = record_str(
        &promotion,
        "/playground_cleanup_record_id",
        "promotion-record.json",
    );
    let cleanup_name = format!("playground-cleanup-{}.json", cleanup_id);
    fs::copy(
        root.join(".appsdk").join("records").join(&cleanup_name),
        history_version.join(&cleanup_name),
    )
    .unwrap_or_else(|_| fail("MODULE_VERSION_HISTORY_INCOMPLETE"));
    let versioned_protected = contract_root(root, &project, "/governance/protected_root")
        .join("history-versions")
        .join(module_id)
        .join(from);
    assert_no_symlink_components(root, &versioned_protected, "protected_version_history");
    if versioned_protected.exists() {
        fail("PROTECTED_VERSION_HISTORY_EXISTS");
    }
    fs::create_dir_all(versioned_protected.parent().unwrap())
        .unwrap_or_else(|_| fail("MODULE_VERSION_OPEN_FAILED"));
    fs::rename(&protected_archive, &versioned_protected)
        .unwrap_or_else(|_| fail("MODULE_VERSION_OPEN_FAILED"));

    let mut candidate = project.clone();
    candidate["modules"][index]["stage"] = Value::String("source_implemented".into());
    candidate["modules"][index]["version_base"] = serde_json::json!({
        "previous_active_version": from,
        "new_active_version": to,
        "base_artifact_hash": previous_hash,
        "base_source_commit": record_str(&freeze, "/source_commit_or_tag", "freeze-record.json")
    });
    write_project(root, &candidate);
    println!(
        "{}",
        serde_json::to_string_pretty(&candidate["modules"][index]["version_base"]).unwrap()
    );
}

fn stage_protected_archive(
    root: &Path,
    project: &Value,
    module: &Value,
    module_id: &str,
    artifact: &Value,
    freeze: &Value,
    staging_archive: &Path,
) {
    assert_no_symlink_components(root, staging_archive, "protected_archive_staging");
    if staging_archive.exists() {
        fail(format!("PROTECTED_ARCHIVE_STAGING_EXISTS:{}", module_id));
    }
    for path in module
        .get("owned_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULE_SURFACES"))
    {
        let relative = path
            .as_str()
            .unwrap_or_else(|| fail("INVALID_MODULE_SURFACES"));
        let source = safe_owned_path(root, relative, "owned_path");
        if !source.exists() {
            fail("PROTECTED_ARCHIVE_SOURCE_MISSING");
        }
    }
    for path in module
        .get("contract_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULE_SURFACES"))
    {
        let relative = path
            .as_str()
            .unwrap_or_else(|| fail("INVALID_MODULE_SURFACES"));
        let source = safe_owned_path(root, relative, "contract_path");
        if !source.exists() {
            fail("PROTECTED_ARCHIVE_CONTRACT_MISSING");
        }
    }
    for entry in artifact
        .get("artifacts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
    {
        let relative = record_str(entry, "/path", "module-artifact-entry");
        let expected = record_str(entry, "/hash", "module-artifact-entry");
        if file_sha256(
            &safe_module_artifact_path(root, project, module_id, relative),
            "protected_library_source",
        ) != expected
        {
            fail("PROTECTED_ARCHIVE_LIBRARY_HASH_MISMATCH");
        }
    }

    fs::create_dir_all(staging_archive).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
    atomic_write_json(
        &staging_archive.join("freeze-artifact.json"),
        artifact,
        "PROTECTED_ARCHIVE_FAILED",
    );
    atomic_write_json(
        &staging_archive.join("module-artifact.json"),
        artifact,
        "PROTECTED_ARCHIVE_FAILED",
    );
    atomic_write_json(
        &staging_archive.join("module-contract.json"),
        module,
        "PROTECTED_ARCHIVE_FAILED",
    );
    for path in module
        .get("owned_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULE_SURFACES"))
    {
        let relative = path
            .as_str()
            .unwrap_or_else(|| fail("INVALID_MODULE_SURFACES"));
        let source = safe_owned_path(root, relative, "owned_path");
        let target = staging_archive
            .join("source")
            .join(relative.trim_end_matches("/**").trim_end_matches('/'));
        if relative.ends_with("/**") {
            copy_tree(&source, &target);
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
            }
            fs::copy(&source, target).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
        }
    }
    for path in module
        .get("contract_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULE_SURFACES"))
    {
        let relative = path
            .as_str()
            .unwrap_or_else(|| fail("INVALID_MODULE_SURFACES"));
        let source = safe_owned_path(root, relative, "contract_path");
        let archive_relative = relative
            .trim_start_matches("contracts/")
            .trim_start_matches("contracts")
            .trim_start_matches('/');
        let target = staging_archive.join("contracts").join(
            archive_relative
                .trim_end_matches("/**")
                .trim_end_matches('/'),
        );
        if relative.ends_with("/**") {
            copy_tree(&source, &target);
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
            }
            fs::copy(&source, target).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
        }
    }
    for entry in artifact
        .get("artifacts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
    {
        let relative = record_str(entry, "/path", "module-artifact-entry");
        let source = safe_module_artifact_path(root, project, module_id, relative);
        let target = staging_archive.join("library").join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
        }
        fs::copy(&source, target).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
    }
    atomic_write_json(
        &staging_archive.join("source-snapshot.json"),
        &serde_json::json!({
            "module_id": module_id,
            "source_commit_or_tag": record_str(
                freeze,
                "/source_commit_or_tag",
                &freeze_record_name(module_id),
            )
        }),
        "PROTECTED_ARCHIVE_FAILED",
    );
}

fn assert_protected_archive_matches(root: &Path, module: &Value, artifact: &Value, archive: &Path) {
    assert_no_symlink_components(root, archive, "protected_archive");
    let archived: Value = serde_json::from_str(
        &fs::read_to_string(archive.join("module-artifact.json"))
            .unwrap_or_else(|_| fail("MODULE_ARTIFACT_HISTORY_MISSING")),
    )
    .unwrap_or_else(|_| fail("INVALID_MODULE_ARTIFACT"));
    previous_active_matches_module(module, &archived);
    if record_str(&archived, "/artifact_hash", "module-artifact")
        != record_str(artifact, "/artifact_hash", "module-artifact")
    {
        fail("PROTECTED_ARCHIVE_ARTIFACT_HASH_MISMATCH");
    }
    for entry in archived
        .get("artifacts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
    {
        let relative = record_str(entry, "/path", "module-artifact-entry");
        let expected = record_str(entry, "/hash", "module-artifact-entry");
        let library_root = archive.join("library");
        let source = safe_owned_path(&library_root, relative, "protected_library");
        if file_sha256(&source, "protected_library") != expected {
            fail("PROTECTED_ARCHIVE_LIBRARY_HASH_MISMATCH");
        }
    }
}

fn restore_active_from_archive(
    root: &Path,
    project: &Value,
    module: &Value,
    module_id: &str,
    version: &str,
    archive: &Path,
) {
    assert_version(version, "INVALID_ACTIVE_VERSION");
    let artifact: Value = serde_json::from_str(
        &fs::read_to_string(archive.join("module-artifact.json"))
            .unwrap_or_else(|_| fail("MODULE_ARTIFACT_HISTORY_MISSING")),
    )
    .unwrap_or_else(|_| fail("INVALID_MODULE_ARTIFACT"));
    previous_active_matches_module(module, &artifact);
    let active_root = contract_root(root, project, "/governance/active_root");
    let active = active_root.join(module_id).join(version);
    let index = active_root.join(module_id).join("current.json");
    if active.exists() {
        fail(format!("ACTIVE_VERSION_EXISTS:{}", version));
    }
    let staging = generated_root(root, project)
        .join("active-restore")
        .join(format!("{}.{}", module_id, std::process::id()));
    assert_no_symlink_components(root, &staging, "active_restore_staging");
    if staging.exists() {
        fail("ACTIVE_RESTORE_STAGING_EXISTS");
    }
    for entry in artifact
        .get("artifacts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
    {
        let relative = record_str(entry, "/path", "module-artifact-entry");
        let expected = record_str(entry, "/hash", "module-artifact-entry");
        let library_root = archive.join("library");
        let source = safe_owned_path(&library_root, relative, "protected_library");
        if file_sha256(&source, "protected_library") != expected {
            fail("PROTECTED_ARCHIVE_LIBRARY_HASH_MISMATCH");
        }
    }
    fs::create_dir_all(staging.join("lib")).unwrap_or_else(|_| fail("ACTIVE_RESTORE_FAILED"));
    atomic_write_json(
        &staging.join("artifact.json"),
        &artifact,
        "ACTIVE_RESTORE_FAILED",
    );
    for entry in artifact
        .get("artifacts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
    {
        let relative = record_str(entry, "/path", "module-artifact-entry");
        let library_root = archive.join("library");
        let source = safe_owned_path(&library_root, relative, "protected_library");
        let target = staging.join("lib").join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|_| fail("ACTIVE_RESTORE_FAILED"));
        }
        fs::copy(source, target).unwrap_or_else(|_| fail("ACTIVE_RESTORE_FAILED"));
    }
    fs::create_dir_all(
        active
            .parent()
            .unwrap_or_else(|| fail("ACTIVE_RESTORE_FAILED")),
    )
    .unwrap_or_else(|_| fail("ACTIVE_RESTORE_FAILED"));
    fs::rename(&staging, &active).unwrap_or_else(|_| fail("ACTIVE_RESTORE_FAILED"));
    atomic_write_json(
        &index,
        &serde_json::json!({
            "module_id": module_id,
            "version": version,
            "artifact_hash": record_str(&artifact, "/artifact_hash", "module-artifact")
        }),
        "ACTIVE_RESTORE_FAILED",
    );
}

fn rehydrate_transaction_dir(root: &Path, module_id: &str) -> PathBuf {
    root.join(".appsdk")
        .join("transactions")
        .join(format!("rehydrate-{}", module_id))
}

fn read_rehydrate_transaction(
    root: &Path,
    module_id: &str,
    version: &str,
    artifact_hash: &str,
) -> Option<Value> {
    let transaction = rehydrate_transaction_dir(root, module_id);
    if !transaction.exists() {
        return None;
    }
    assert_no_symlink_components(root, &transaction, "rehydrate_transaction");
    let marker: Value = serde_json::from_str(
        &fs::read_to_string(transaction.join("marker.json"))
            .unwrap_or_else(|_| fail("FROZEN_REHYDRATE_TRANSACTION_MARKER_MISSING")),
    )
    .unwrap_or_else(|_| fail("INVALID_FROZEN_REHYDRATE_TRANSACTION"));
    if marker.get("schema_version").and_then(Value::as_u64) != Some(1)
        || marker.get("module_id").and_then(Value::as_str) != Some(module_id)
        || marker.get("version").and_then(Value::as_str) != Some(version)
        || marker.get("artifact_hash").and_then(Value::as_str) != Some(artifact_hash)
        || !matches!(
            marker.get("phase").and_then(Value::as_str),
            Some(
                "prepared"
                    | "previous_active_restored"
                    | "protected_ready"
                    | "active_published"
                    | "verified"
            )
        )
        || DateTime::parse_from_rfc3339(record_str(&marker, "/created_at", "rehydrate-transaction"))
            .is_err()
    {
        fail("FROZEN_REHYDRATE_TRANSACTION_MISMATCH");
    }
    Some(marker)
}

fn write_rehydrate_transaction(
    root: &Path,
    module_id: &str,
    version: &str,
    artifact_hash: &str,
    phase: &str,
) {
    if !matches!(
        phase,
        "prepared"
            | "previous_active_restored"
            | "protected_ready"
            | "active_published"
            | "verified"
    ) {
        fail("INVALID_FROZEN_REHYDRATE_TRANSACTION_PHASE");
    }
    let transaction = rehydrate_transaction_dir(root, module_id);
    assert_no_symlink_components(root, &transaction, "rehydrate_transaction");
    let existing = read_rehydrate_transaction(root, module_id, version, artifact_hash);
    fs::create_dir_all(&transaction)
        .unwrap_or_else(|_| fail("FROZEN_REHYDRATE_TRANSACTION_WRITE_FAILED"));
    let created_at = existing
        .and_then(|marker| marker.get("created_at").cloned())
        .unwrap_or_else(|| Value::String(Utc::now().to_rfc3339()));
    atomic_write_json(
        &transaction.join("marker.json"),
        &serde_json::json!({
            "schema_version": 1,
            "module_id": module_id,
            "version": version,
            "artifact_hash": artifact_hash,
            "phase": phase,
            "created_at": created_at
        }),
        "FROZEN_REHYDRATE_TRANSACTION_WRITE_FAILED",
    );
}

fn active_version_projection_matches(
    root: &Path,
    project: &Value,
    module_id: &str,
    version: &str,
    artifact: &Value,
) -> bool {
    let active = contract_root(root, project, "/governance/active_root")
        .join(module_id)
        .join(version);
    assert_no_symlink_components(root, &active, "active_projection");
    if !active.is_dir() {
        return false;
    }
    let active_artifact: Value = match fs::read_to_string(active.join("artifact.json"))
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
    {
        Some(value) => value,
        None => return false,
    };
    if active_artifact != *artifact {
        return false;
    }
    artifact
        .get("artifacts")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries.iter().all(|entry| {
                let relative = record_str(entry, "/path", "module-artifact-entry");
                let expected = record_str(entry, "/hash", "module-artifact-entry");
                let target = active.join("lib").join(relative);
                target.is_file() && file_sha256(&target, "active_projection") == expected
            })
        })
}

fn assert_previous_active_projection_matches(
    root: &Path,
    project: &Value,
    module: &Value,
    module_id: &str,
    version: &str,
    archive: &Path,
) {
    let artifact: Value = serde_json::from_str(
        &fs::read_to_string(archive.join("module-artifact.json"))
            .unwrap_or_else(|_| fail("MODULE_ARTIFACT_HISTORY_MISSING")),
    )
    .unwrap_or_else(|_| fail("INVALID_MODULE_ARTIFACT"));
    previous_active_matches_module(module, &artifact);
    if !active_version_projection_matches(root, project, module_id, version, &artifact) {
        fail("FROZEN_REHYDRATE_PREVIOUS_ACTIVE_MISMATCH");
    }
}

fn assert_active_projection_matches(
    root: &Path,
    project: &Value,
    module_id: &str,
    version: &str,
    artifact: &Value,
) {
    if !active_version_projection_matches(root, project, module_id, version, artifact) {
        fail("FROZEN_REHYDRATE_ACTIVE_PROJECTION_MISMATCH");
    }
    let index = contract_root(root, project, "/governance/active_root")
        .join(module_id)
        .join("current.json");
    assert_no_symlink_components(root, &index, "active_index");
    let current: Value = serde_json::from_str(
        &fs::read_to_string(index).unwrap_or_else(|_| fail("ACTIVE_INDEX_MISSING")),
    )
    .unwrap_or_else(|_| fail("INVALID_ACTIVE_INDEX"));
    if current.get("module_id").and_then(Value::as_str) != Some(module_id)
        || current.get("version").and_then(Value::as_str) != Some(version)
        || current.get("artifact_hash").and_then(Value::as_str)
            != artifact.get("artifact_hash").and_then(Value::as_str)
    {
        fail("FROZEN_REHYDRATE_ACTIVE_PROJECTION_MISMATCH");
    }
}

fn finish_rehydrate_transaction(root: &Path, module_id: &str) {
    let transaction = rehydrate_transaction_dir(root, module_id);
    let marker: Value = serde_json::from_str(
        &fs::read_to_string(transaction.join("marker.json"))
            .unwrap_or_else(|_| fail("FROZEN_REHYDRATE_TRANSACTION_MARKER_MISSING")),
    )
    .unwrap_or_else(|_| fail("INVALID_FROZEN_REHYDRATE_TRANSACTION"));
    if marker.get("phase").and_then(Value::as_str) != Some("verified") {
        fail("INVALID_FROZEN_REHYDRATE_TRANSACTION_PHASE");
    }
    fs::remove_dir_all(transaction)
        .unwrap_or_else(|_| fail("FROZEN_REHYDRATE_TRANSACTION_CLEANUP_FAILED"));
}

fn rehydrate_frozen(root: &Path, module_id: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_project_contract(root, &project);
    assert_declared_contracts(root, &project, true);
    assert_goal_confirmed(root);
    assert_sdk_lock(root, &project, true);
    let module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    if module.get("stage").and_then(Value::as_str) != Some("frozen") {
        fail(format!("FROZEN_REHYDRATE_REQUIRES_FROZEN:{}", module_id));
    }
    let freeze_name = freeze_record_name(module_id);
    let freeze = read_record(root, &freeze_name);
    let version = record_str(&freeze, "/active_version", &freeze_name);
    assert_version(version, "INVALID_ACTIVE_VERSION");
    let promotion = read_record(root, &module_record_name("promotion-record", module_id));
    let protected_root = contract_root(root, &project, "/governance/protected_root");
    let archive = protected_root.join("history").join(module_id);
    assert_no_symlink_components(root, &archive, "protected_archive");
    assert_protected_not_ignored(root, &archive);
    let active_root = contract_root(root, &project, "/governance/active_root");

    run_module_build(root, module, module_id);
    let artifact = build_module_artifact(root, &project, module, module_id);
    let artifact_hash = record_str(&artifact, "/artifact_hash", "module-artifact");
    if artifact_hash != record_str(&freeze, "/library_hash", &freeze_name)
        || artifact_hash != record_str(&promotion, "/artifact_hash", "promotion-record.json")
    {
        fail("FROZEN_REHYDRATE_ARTIFACT_HASH_MISMATCH");
    }
    let transaction = read_rehydrate_transaction(root, module_id, version, artifact_hash);
    let previous_version = freeze
        .get("previous_active_version")
        .and_then(Value::as_str);
    let current_active = active_root.join(module_id).join(version);
    let current_index = active_root.join(module_id).join("current.json");
    let index_version = if current_index.exists() {
        let index: Value = serde_json::from_str(
            &fs::read_to_string(&current_index).unwrap_or_else(|_| fail("INVALID_ACTIVE_INDEX")),
        )
        .unwrap_or_else(|_| fail("INVALID_ACTIVE_INDEX"));
        if index.get("module_id").and_then(Value::as_str) != Some(module_id) {
            fail("INVALID_ACTIVE_INDEX");
        }
        Some(record_str(&index, "/version", "active-index").to_string())
    } else {
        None
    };
    if index_version.as_deref().is_some_and(|index_version| {
        index_version != version && previous_version != Some(index_version)
    }) {
        if transaction.is_some() {
            fail("FROZEN_REHYDRATE_TRANSACTION_PROJECTION_MISMATCH");
        }
        fail("FROZEN_REHYDRATE_UNOWNED_PARTIAL_PROJECTION");
    }
    let index_targets_current = index_version.as_deref() == Some(version);
    let current_projection_present = current_active.exists() || index_targets_current;
    let current_projection_complete = archive.is_dir()
        && current_active.is_dir()
        && index_targets_current
        && active_version_projection_matches(root, &project, module_id, version, &artifact);
    if transaction.is_none() && current_projection_present && !current_projection_complete {
        fail("FROZEN_REHYDRATE_UNOWNED_PARTIAL_PROJECTION");
    }
    if transaction.is_some() && current_projection_present && !current_projection_complete {
        fail("FROZEN_REHYDRATE_TRANSACTION_PROJECTION_MISMATCH");
    }

    if transaction.is_none() && current_projection_complete {
        write_module_artifact_value(root, &project, module_id, &artifact);
        write_artifact(root, &project);
        assert_record_graph(root, Some(module_id), &artifact, true);
        assert_protected_archive_matches(root, module, &artifact, &archive);
        assert_active_projection_matches(root, &project, module_id, version, &artifact);
        if let Some(previous) = previous_version {
            let previous_archive = protected_root
                .join("history-versions")
                .join(module_id)
                .join(previous);
            if !previous_archive.is_dir() {
                fail("PROTECTED_VERSION_HISTORY_MISSING");
            }
            assert_previous_active_projection_matches(
                root,
                &project,
                module,
                module_id,
                previous,
                &previous_archive,
            );
        }
        verify_internal(root, false, false);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "module_id": module_id,
                "version": version,
                "artifact_hash": artifact_hash,
                "rehydrated": true,
                "already_complete": true
            }))
            .unwrap()
        );
        return;
    }
    if previous_version.is_none() {
        write_module_artifact_value(root, &project, module_id, &artifact);
        write_artifact(root, &project);
        assert_record_graph(root, Some(module_id), &artifact, true);
    }
    if transaction.is_none() {
        write_rehydrate_transaction(root, module_id, version, artifact_hash, "prepared");
    }

    if let Some(previous) = previous_version {
        let previous_active = active_root.join(module_id).join(previous);
        let previous_archive = protected_root
            .join("history-versions")
            .join(module_id)
            .join(previous);
        if !previous_archive.is_dir() {
            fail("PROTECTED_VERSION_HISTORY_MISSING");
        }
        if previous_active.is_dir() {
            assert_previous_active_projection_matches(
                root,
                &project,
                module,
                module_id,
                previous,
                &previous_archive,
            );
        } else {
            if current_active.exists() {
                fail("FROZEN_REHYDRATE_TRANSACTION_PROJECTION_MISMATCH");
            }
            restore_active_from_archive(
                root,
                &project,
                module,
                module_id,
                previous,
                &previous_archive,
            );
        }
        write_rehydrate_transaction(
            root,
            module_id,
            version,
            artifact_hash,
            "previous_active_restored",
        );
    }

    if previous_version.is_some() {
        write_module_artifact_value(root, &project, module_id, &artifact);
        write_artifact(root, &project);
        assert_record_graph(root, Some(module_id), &artifact, true);
    }

    if archive.exists() {
        assert_protected_archive_matches(root, module, &artifact, &archive);
    } else {
        let staging =
            archive.with_file_name(format!(".{}.rehydrate.{}", module_id, std::process::id()));
        stage_protected_archive(
            root, &project, module, module_id, &artifact, &freeze, &staging,
        );
        fs::rename(&staging, &archive).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
    }
    write_rehydrate_transaction(root, module_id, version, artifact_hash, "protected_ready");
    if current_active.exists() {
        assert_active_projection_matches(root, &project, module_id, version, &artifact);
    } else {
        publish_active(root, module_id, version);
    }
    write_rehydrate_transaction(root, module_id, version, artifact_hash, "active_published");
    verify_internal(root, false, false);
    write_rehydrate_transaction(root, module_id, version, artifact_hash, "verified");
    finish_rehydrate_transaction(root, module_id);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "module_id": module_id,
            "version": version,
            "artifact_hash": artifact_hash,
            "rehydrated": true
        }))
        .unwrap()
    );
}

fn freeze_transaction_dir(root: &Path, module_id: &str) -> PathBuf {
    root.join(".appsdk")
        .join("transactions")
        .join(format!("freeze-{}", module_id))
}

fn recover_freeze_transaction(root: &Path, project: &Value, module_id: &str) -> bool {
    let transaction = freeze_transaction_dir(root, module_id);
    if !transaction.exists() {
        return false;
    }
    let marker: Value = serde_json::from_str(
        &fs::read_to_string(transaction.join("marker.json"))
            .unwrap_or_else(|_| fail("FREEZE_TRANSACTION_MARKER_MISSING")),
    )
    .unwrap_or_else(|_| fail("INVALID_FREEZE_TRANSACTION_MARKER"));
    let phase = marker
        .get("phase")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("INVALID_FREEZE_TRANSACTION_MARKER"));
    let protected_root = contract_root(root, project, "/governance/protected_root");
    let archive = protected_root.join("history").join(module_id);
    let staging = protected_root.join("history").join(format!(
        ".{}.staging.{}",
        module_id,
        marker["pid"].as_u64().unwrap_or(0)
    ));
    if phase == "commit_ready"
        || project
            .pointer("/modules")
            .and_then(Value::as_array)
            .and_then(|modules| {
                modules.iter().find(|module| {
                    module.get("module_id").and_then(Value::as_str) == Some(module_id)
                })
            })
            .and_then(|module| module.get("stage"))
            .and_then(Value::as_str)
            == Some("frozen")
    {
        if !archive.exists() {
            fs::rename(&staging, &archive)
                .unwrap_or_else(|_| fail("FREEZE_TRANSACTION_RECOVERY_FAILED"));
        }
        fs::remove_dir_all(&transaction)
            .unwrap_or_else(|_| fail("FREEZE_TRANSACTION_CLEANUP_FAILED"));
        return true;
    }
    if phase == "prepared" {
        let backup = transaction.join("backup");
        for (name, target) in [
            ("project.json", root.join(".appsdk/project.json")),
            (
                "project.compiled.json",
                generated_root(root, project).join("project.compiled.json"),
            ),
            (
                "module.compiled.json",
                module_artifact_file(root, project, module_id),
            ),
            (
                "review-record.json",
                root.join(".appsdk/records")
                    .join(module_record_name("review-record", module_id)),
            ),
            (
                "promotion-record.json",
                root.join(".appsdk/records")
                    .join(module_record_name("promotion-record", module_id)),
            ),
            (
                "regression-report.json",
                root.join(".appsdk/records")
                    .join(module_record_name("regression-report", module_id)),
            ),
            (
                "freeze-record.json",
                root.join(".appsdk/records")
                    .join(freeze_record_name(module_id)),
            ),
        ] {
            fs::copy(backup.join(name), target)
                .unwrap_or_else(|_| fail("FREEZE_TRANSACTION_ROLLBACK_FAILED"));
        }
        if staging.exists() {
            fs::remove_dir_all(&staging)
                .unwrap_or_else(|_| fail("FREEZE_TRANSACTION_ROLLBACK_FAILED"));
        }
        fs::remove_dir_all(&transaction)
            .unwrap_or_else(|_| fail("FREEZE_TRANSACTION_CLEANUP_FAILED"));
        return false;
    }
    fail("INVALID_FREEZE_TRANSACTION_PHASE");
}

fn atomic_write_bytes(target: &Path, bytes: &[u8], error: &str) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| fail("STAGING_NONCE_FAILED"))
        .as_nanos();
    let staging = target.with_extension(format!("staging.{}.{}", std::process::id(), nonce));
    if fs::symlink_metadata(&staging)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:staging");
    }
    fs::write(&staging, bytes).unwrap_or_else(|_| fail(error));
    fs::rename(&staging, target).unwrap_or_else(|_| fail(error));
}

fn atomic_write_json(target: &Path, value: &Value, error: &str) {
    atomic_write_bytes(
        target,
        (serde_json::to_string_pretty(value).unwrap() + "\n").as_bytes(),
        error,
    );
}

fn read_record(root: &Path, name: &str) -> Value {
    let file = root.join(".appsdk").join("records").join(name);
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(format!("GOVERNANCE_PATH_SYMLINK:record:{}", name));
    }
    serde_json::from_str(
        &fs::read_to_string(&file).unwrap_or_else(|_| fail(format!("MISSING_RECORD:{}", name))),
    )
    .unwrap_or_else(|_| fail(format!("INVALID_RECORD:{}", name)))
}

fn write_record(root: &Path, name: &str, record: &Value) {
    assert_no_symlink_components(
        root,
        &root.join(".appsdk").join("records"),
        "record_control",
    );
    let target = root.join(".appsdk").join("records").join(name);
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:record");
    }
    atomic_write_json(&target, record, &format!("RECORD_WRITE_FAILED:{}", name));
}

fn record_str<'a>(record: &'a Value, path: &str, name: &str) -> &'a str {
    record
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail(format!("INVALID_RECORD:{}:{}", name, path)))
}

fn record_array<'a>(record: &'a Value, path: &str, name: &str) -> &'a Vec<Value> {
    record
        .pointer(path)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| fail(format!("INVALID_RECORD:{}:{}", name, path)))
}

fn assert_record_schema(evidence: &Value, review: &Value, promotion: &Value) {
    for (record, name, fields) in [
        (
            evidence,
            "evidence-record.json",
            &[
                "evidence_id",
                "issue_id",
                "experiment_id",
                "phase",
                "source_commit",
                "created_at",
                "expires_at",
                "scope_hash",
            ][..],
        ),
        (
            review,
            "review-record.json",
            &[
                "review_id",
                "issue_id",
                "promotion_id",
                "review_kind",
                "fix_candidate_id",
                "reviewed_commit",
                "reviewed_tree_hash",
                "reviewed_diff_hash",
                "reviewed_artifact_hash",
                "reviewed_scope_hash",
                "resource_map_hash",
                "function_map_hash",
                "mainline_call_map_hash",
                "verification_map_hash",
                "confidence_rationale",
                "created_at",
            ][..],
        ),
        (
            promotion,
            "promotion-record.json",
            &[
                "promotion_id",
                "issue_id",
                "experiment_id",
                "module_id",
                "base_commit",
                "source_commit",
                "candidate_commit",
                "merged_commit",
                "new_active_version",
                "review_id",
                "worktree_record_id",
                "reproduction_record_id",
                "fix_candidate_id",
                "architecture_review_id",
                "effectiveness_record_id",
                "merge_record_id",
                "root_cause",
                "design_id",
                "change_reason_comment",
                "playground_cleanup_record_id",
                "created_at",
            ][..],
        ),
    ] {
        for field in fields {
            record_str(record, &format!("/{}", field), name);
        }
    }
    assert_evidence_record(evidence, "evidence-record.json", Utc::now());
    for path in [
        "/reviewer/adapter",
        "/reviewer/identity",
        "/verdict",
        "/reviewed_commit",
        "/reviewed_artifact_hash",
        "/reviewed_scope_hash",
        "/confidence_rationale",
        "/created_at",
    ] {
        record_str(review, path, "review-record.json");
    }
    for path in [
        "/base_commit",
        "/new_active_version",
        "/review_id",
        "/change_set_id",
        "/compatibility_level",
        "/root_cause",
        "/design_id",
        "/change_reason_comment",
        "/playground_cleanup_record_id",
        "/created_at",
    ] {
        record_str(promotion, path, "promotion-record.json");
    }
    if !matches!(
        promotion.get("compatibility_level").and_then(Value::as_str),
        Some("compatible" | "migration_required" | "breaking")
    ) {
        fail("INVALID_PROMOTION_RECORD");
    }
    let reviewer = review
        .get("reviewer")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("INVALID_REVIEW_RECORD"));
    for key in ["adapter", "identity"] {
        if reviewer
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            fail("INVALID_REVIEW_RECORD");
        }
    }
    if review
        .get("evidence_ids")
        .and_then(Value::as_array)
        .map(|values| values.is_empty() || values.iter().any(|value| value.as_str().is_none()))
        .unwrap_or(true)
    {
        fail("INVALID_REVIEW_RECORD");
    }
    if !promotion
        .get("previous_active_version")
        .map(|value| value.is_null() || value.as_str().is_some())
        .unwrap_or(false)
        || promotion
            .get("evidence_ids")
            .and_then(Value::as_array)
            .map(|values| values.is_empty() || values.iter().any(|value| value.as_str().is_none()))
            .unwrap_or(true)
        || promotion
            .get("artifact_hash")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        || promotion
            .get("scope_hash")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        || promotion
            .get("public_api_hash")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
    {
        fail("INVALID_PROMOTION_RECORD");
    }
    let gates = promotion
        .get("required_gate_results")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| fail("INVALID_PROMOTION_RECORD"));
    for gate in gates {
        for key in ["gate_id", "producer"] {
            if gate
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                fail("INVALID_PROMOTION_RECORD");
            }
        }
        if gate.get("result").and_then(Value::as_str).is_none() {
            fail("INVALID_PROMOTION_RECORD");
        }
    }
}

fn assert_regression_report(
    root: &Path,
    module_id: &str,
    module: &Value,
    promotion: &Value,
    artifact: &Value,
) -> (Value, String) {
    let name = module_record_name("regression-report", module_id);
    let report = read_record(root, &name);
    for path in [
        "/regression_report_id",
        "/module_id",
        "/source_commit",
        "/artifact_hash",
        "/public_api_hash",
        "/scope_hash",
        "/input_hash",
        "/suite_id",
        "/command/program",
        "/command/working_directory",
        "/producer/adapter",
        "/producer/identity",
        "/created_at",
    ] {
        record_str(&report, path, &name);
    }
    let tc = report
        .get("test_characteristics")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail(format!("INVALID_REGRESSION_REPORT:{}", name)));
    if tc.get("whitebox") != Some(&Value::Bool(true))
        || tc.get("blackbox") != Some(&Value::Bool(true))
    {
        fail(format!("INVALID_REGRESSION_REPORT:{}", name));
    }
    let policy = module
        .get("regression")
        .unwrap_or_else(|| fail(format!("INVALID_REGRESSION_CONTRACT:{}", module_id)));
    if record_str(&report, "/module_id", &name) != module_id
        || record_str(&report, "/source_commit", &name)
            != record_str(promotion, "/source_commit", "promotion-record.json")
        || record_str(&report, "/artifact_hash", &name)
            != record_str(artifact, "/artifact_hash", "artifact")
        || record_str(&report, "/public_api_hash", &name)
            != record_str(promotion, "/public_api_hash", "promotion-record.json")
        || record_str(&report, "/scope_hash", &name)
            != record_str(promotion, "/scope_hash", "promotion-record.json")
        || record_str(&report, "/input_hash", &name)
            != record_str(artifact, "/artifact_hash", "artifact")
        || record_str(&report, "/suite_id", &name)
            != record_str(policy, "/suite_id", "regression-policy")
        || report.get("command") != policy.get("command")
    {
        fail("REGRESSION_REPORT_INPUT_MISMATCH");
    }
    let test_count = report
        .get("test_count")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("INVALID_REGRESSION_REPORT"));
    let passed = report
        .get("passed")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("INVALID_REGRESSION_REPORT"));
    let failed = report
        .get("failed")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("INVALID_REGRESSION_REPORT"));
    let skipped = report
        .get("skipped")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("INVALID_REGRESSION_REPORT"));
    let minimum = policy
        .get("minimum_test_count")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("INVALID_REGRESSION_CONTRACT"));
    if report.get("result").and_then(Value::as_str) != Some("pass")
        || test_count < minimum
        || passed != test_count
        || failed != 0
        || (!policy
            .get("allow_skipped")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && skipped != 0)
    {
        fail("REGRESSION_REPORT_NOT_PASSED");
    }
    let report_hash = sha256(&canonical(&report));
    (report, report_hash)
}

fn git_value(root: &Path, args: &[&str], error: &str) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap_or_else(|_| fail(error));
    if !output.status.success() {
        fail(error);
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn assert_mutation_worktree(root: &Path) {
    let output = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "symbolic-ref",
            "--quiet",
            "--short",
            "HEAD",
        ])
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "main" {
        fail("MAIN_WORKTREE_MUTATION_FORBIDDEN");
    }
}

fn assert_candidate_source_identity(root: &Path, module: &Value, candidate_commit: &str) {
    let mut controlled_paths = Vec::new();
    for key in ["owned_paths", "contract_paths"] {
        for value in record_array(module, &format!("/{}", key), "module") {
            controlled_paths.push(
                value
                    .as_str()
                    .unwrap_or_else(|| fail("INVALID_MODULE_CONTROLLED_PATH")),
            );
        }
    }
    let diff_status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--quiet", candidate_commit, "--"])
        .args(&controlled_paths)
        .status()
        .unwrap_or_else(|_| fail("CANDIDATE_SOURCE_GIT_UNAVAILABLE"));
    if !diff_status.success() {
        fail("CANDIDATE_CONTROLLED_SOURCE_DRIFT");
    }
    let untracked = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--others", "--exclude-standard", "--"])
        .args(&controlled_paths)
        .output()
        .unwrap_or_else(|_| fail("CANDIDATE_SOURCE_GIT_UNAVAILABLE"));
    if !untracked.status.success() || !untracked.stdout.is_empty() {
        fail("CANDIDATE_CONTROLLED_SOURCE_DRIFT");
    }
}

fn git_ls_remote(root: &Path, remote: &str, remote_ref: &str) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-remote", remote, remote_ref])
        .output()
        .unwrap_or_else(|_| fail("REMOTE_ADAPTER_UNAVAILABLE"));
    if !output.status.success() {
        fail("REMOTE_MAIN_QUERY_FAILED");
    }
    String::from_utf8(output.stdout)
        .ok()
        .and_then(|value| value.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| fail("REMOTE_MAIN_REF_MISSING"))
}

fn record_time(record: &Value, name: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(record_str(record, "/created_at", name))
        .unwrap_or_else(|_| fail(format!("INVALID_RECORD_TIME:{}", name)))
        .with_timezone(&Utc)
}

fn record_datetime(record: &Value, path: &str, name: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(record_str(record, path, name))
        .unwrap_or_else(|_| fail(format!("INVALID_RECORD_TIME:{}:{}", name, path)))
        .with_timezone(&Utc)
}

fn assert_evidence_record(evidence: &Value, name: &str, admission_time: DateTime<Utc>) {
    for path in [
        "/evidence_id",
        "/issue_id",
        "/experiment_id",
        "/phase",
        "/kind",
        "/source_commit",
        "/result",
        "/created_at",
        "/expires_at",
        "/scope_hash",
        "/scope/module_id",
        "/producer/adapter",
        "/producer/identity",
    ] {
        record_str(evidence, path, name);
    }
    if !matches!(
        evidence.get("kind").and_then(Value::as_str),
        Some(
            "red_test"
                | "positive_test"
                | "negative_test"
                | "sample_replay"
                | "build"
                | "install"
                | "restart"
                | "artifact"
                | "runtime"
                | "gate"
        )
    ) || evidence.get("result").and_then(Value::as_str) != Some("pass")
        || evidence
            .get("input_hashes")
            .and_then(Value::as_array)
            .map(|values| values.iter().any(|value| value.as_str().is_none()))
            .unwrap_or(true)
    {
        fail(format!("INVALID_EVIDENCE_RECORD:{}", name));
    }
    let created_at = record_time(evidence, name);
    let expires_at = record_datetime(evidence, "/expires_at", name);
    if created_at > expires_at || admission_time > expires_at {
        fail(format!("EXPIRED_EVIDENCE_RECORD:{}", name));
    }
}

fn evidence_by_id(root: &Path, module_id: &str, evidence_id: &str) -> Value {
    assert_identifier(evidence_id, "INVALID_EVIDENCE_ID");
    let relative = format!(
        ".appsdk/records/evidence/{}/{}.json",
        module_id, evidence_id
    );
    let file = safe_owned_path(root, &relative, "evidence_record");
    serde_json::from_str(
        &fs::read_to_string(&file)
            .unwrap_or_else(|_| fail(format!("MISSING_EVIDENCE_RECORD:{}", evidence_id))),
    )
    .unwrap_or_else(|_| fail(format!("INVALID_EVIDENCE_RECORD:{}", evidence_id)))
}

fn deployment_receipt_time(
    root: &Path,
    module_id: &str,
    evidence_id: &str,
    expected_phase: &str,
    expected_kind: &str,
    issue_id: &str,
    scope_hash: &str,
    candidate_commit: &str,
    artifact_hash: &str,
    environment_id: &str,
    entrypoint: &str,
    producer: &Value,
) -> DateTime<Utc> {
    let evidence = evidence_by_id(root, module_id, evidence_id);
    assert_evidence_record(&evidence, evidence_id, Utc::now());
    if record_str(&evidence, "/issue_id", evidence_id) != issue_id
        || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
        || record_str(&evidence, "/scope_hash", evidence_id) != scope_hash
        || record_str(&evidence, "/source_commit", evidence_id) != candidate_commit
        || record_str(&evidence, "/artifact_hash", evidence_id) != artifact_hash
        || record_str(&evidence, "/phase", evidence_id) != expected_phase
        || record_str(&evidence, "/kind", evidence_id) != expected_kind
        || record_str(&evidence, "/execution_surface", evidence_id) != "deployed_blackbox"
        || record_str(&evidence, "/environment_id", evidence_id) != environment_id
        || record_str(&evidence, "/entrypoint", evidence_id) != entrypoint
        || evidence.get("producer") != Some(producer)
    {
        fail("DEPLOYMENT_RECEIPT_EVIDENCE_MISMATCH");
    }
    record_time(&evidence, evidence_id)
}

fn assert_pre_review_validation_gate(root: &Path, module_id: &str, artifact: &Value) {
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let validation_name = module_record_name("pre-review-validation-record", module_id);
    let candidate = read_record(root, &candidate_name);
    let validation = read_record(root, &validation_name);
    let issue_id = record_str(&candidate, "/issue_id", &candidate_name);
    let scope_hash = record_str(&candidate, "/scope_hash", &candidate_name);
    let candidate_commit = record_str(&candidate, "/head_commit", &candidate_name);
    let candidate_tree = record_str(&candidate, "/tree_hash", &candidate_name);
    let artifact_hash = record_str(artifact, "/artifact_hash", "artifact");
    let project = read_project(root);
    let module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("UNKNOWN_MODULE:{}", module_id)));
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "FIX_CANDIDATE_COMMIT_MISSING",
    ) != candidate_tree
    {
        fail("FIX_CANDIDATE_TREE_MISMATCH");
    }
    assert_candidate_source_identity(root, module, candidate_commit);
    let rebuilt_artifact = build_module_artifact(root, &project, module, module_id);
    if record_str(&rebuilt_artifact, "/artifact_hash", "rebuilt-artifact") != artifact_hash {
        fail("REVIEW_ADMISSION_ARTIFACT_SOURCE_DRIFT");
    }
    if record_str(&validation, "/issue_id", &validation_name) != issue_id
        || record_str(&validation, "/module_id", &validation_name) != module_id
        || record_str(&validation, "/fix_candidate_id", &validation_name)
            != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(&validation, "/candidate_commit", &validation_name) != candidate_commit
        || record_str(&validation, "/candidate_tree_hash", &validation_name) != candidate_tree
        || record_str(&validation, "/artifact_hash", &validation_name) != artifact_hash
        || validation.get("source_unchanged") != Some(&Value::Bool(true))
        || validation.get("result").and_then(Value::as_str) != Some("pass")
    {
        fail("PRE_REVIEW_VALIDATION_MISMATCH");
    }
    let environment_id = record_str(&validation, "/deployment/environment_id", &validation_name);
    let entrypoint = record_str(&validation, "/deployment/entrypoint", &validation_name);
    let producer = validation
        .pointer("/deployment/producer")
        .and_then(Value::as_object)
        .filter(|value| {
            ["adapter", "identity"].iter().all(|key| {
                value
                    .get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|entry| !entry.is_empty())
            })
        })
        .map(|_| validation.pointer("/deployment/producer").unwrap())
        .unwrap_or_else(|| fail("DEPLOYMENT_BLACKBOX_RECEIPT_MISSING"));
    let whitebox_producer = validation
        .get("whitebox_producer")
        .and_then(Value::as_object)
        .filter(|value| {
            ["adapter", "identity"].iter().all(|key| {
                value
                    .get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|entry| !entry.is_empty())
            })
        })
        .map(|_| validation.get("whitebox_producer").unwrap())
        .unwrap_or_else(|| fail("DEVELOPMENT_WHITEBOX_PRODUCER_MISSING"));
    for path in [
        "/deployment/install_receipt_id",
        "/deployment/restart_receipt_id",
        "/deployment/observed_at",
    ] {
        if record_str(&validation, path, &validation_name).is_empty() {
            fail("DEPLOYMENT_BLACKBOX_RECEIPT_MISSING");
        }
    }
    if environment_id.is_empty() || entrypoint.is_empty() {
        fail("DEPLOYMENT_BLACKBOX_RECEIPT_MISSING");
    }
    let install_receipt_id = record_str(
        &validation,
        "/deployment/install_receipt_id",
        &validation_name,
    );
    let restart_receipt_id = record_str(
        &validation,
        "/deployment/restart_receipt_id",
        &validation_name,
    );
    let install_time = deployment_receipt_time(
        root,
        module_id,
        install_receipt_id,
        "deployment_install",
        "install",
        issue_id,
        scope_hash,
        candidate_commit,
        artifact_hash,
        environment_id,
        entrypoint,
        producer,
    );
    let restart_time = deployment_receipt_time(
        root,
        module_id,
        restart_receipt_id,
        "deployment_restart",
        "restart",
        issue_id,
        scope_hash,
        candidate_commit,
        artifact_hash,
        environment_id,
        entrypoint,
        producer,
    );
    let mut all_ids = std::collections::HashSet::new();
    if !all_ids.insert(install_receipt_id) || !all_ids.insert(restart_receipt_id) {
        fail("PRE_REVIEW_EVIDENCE_NOT_DISJOINT");
    }
    let mut latest_whitebox = None;
    let mut earliest_whitebox = None;
    let mut earliest_blackbox = None;
    let mut latest_blackbox = None;
    for (path, phase, surface) in [
        (
            "/whitebox_evidence_ids",
            "development_whitebox",
            "development_whitebox",
        ),
        (
            "/blackbox_evidence_ids",
            "deployed_blackbox",
            "deployed_blackbox",
        ),
    ] {
        for value in record_array(&validation, path, &validation_name) {
            let id = value
                .as_str()
                .unwrap_or_else(|| fail("INVALID_PRE_REVIEW_EVIDENCE_ID"));
            if !all_ids.insert(id) {
                fail("PRE_REVIEW_EVIDENCE_NOT_DISJOINT");
            }
            let evidence = evidence_by_id(root, module_id, id);
            assert_evidence_record(&evidence, id, Utc::now());
            if record_str(&evidence, "/issue_id", id) != issue_id
                || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
                || record_str(&evidence, "/scope_hash", id) != scope_hash
                || record_str(&evidence, "/source_commit", id) != candidate_commit
                || record_str(&evidence, "/phase", id) != phase
                || record_str(&evidence, "/execution_surface", id) != surface
                || evidence.get("result").and_then(Value::as_str) != Some("pass")
                || record_time(&evidence, id) > record_time(&validation, &validation_name)
            {
                fail("PRE_REVIEW_EVIDENCE_MISMATCH");
            }
            if surface == "deployed_blackbox"
                && (record_str(&evidence, "/artifact_hash", id) != artifact_hash
                    || record_str(&evidence, "/environment_id", id) != environment_id
                    || record_str(&evidence, "/entrypoint", id) != entrypoint
                    || evidence.get("producer") != Some(producer)
                    || !matches!(
                        evidence.get("kind").and_then(Value::as_str),
                        Some("runtime" | "sample_replay")
                    ))
            {
                fail("DEPLOYED_BLACKBOX_EVIDENCE_MISMATCH");
            }
            if surface == "development_whitebox"
                && (record_str(&evidence, "/artifact_hash", id) != artifact_hash
                    || evidence.get("producer") != Some(whitebox_producer))
            {
                fail("DEVELOPMENT_WHITEBOX_EVIDENCE_MISMATCH");
            }
            let evidence_time = record_time(&evidence, id);
            if surface == "development_whitebox" {
                earliest_whitebox = Some(match earliest_whitebox {
                    Some(current) if current < evidence_time => current,
                    _ => evidence_time,
                });
                latest_whitebox = Some(match latest_whitebox {
                    Some(current) if current > evidence_time => current,
                    _ => evidence_time,
                });
            } else {
                earliest_blackbox = Some(match earliest_blackbox {
                    Some(current) if current < evidence_time => current,
                    _ => evidence_time,
                });
                latest_blackbox = Some(match latest_blackbox {
                    Some(current) if current > evidence_time => current,
                    _ => evidence_time,
                });
            }
        }
    }
    let observed_at = record_datetime(&validation, "/deployment/observed_at", &validation_name);
    if record_time(&candidate, &candidate_name)
        > earliest_whitebox.unwrap_or_else(|| fail("MISSING_DEVELOPMENT_WHITEBOX_EVIDENCE"))
        || latest_whitebox.unwrap_or_else(|| fail("MISSING_DEVELOPMENT_WHITEBOX_EVIDENCE"))
            > install_time
        || install_time > restart_time
        || restart_time
            > earliest_blackbox.unwrap_or_else(|| fail("MISSING_DEPLOYED_BLACKBOX_EVIDENCE"))
        || latest_blackbox.unwrap_or_else(|| fail("MISSING_DEPLOYED_BLACKBOX_EVIDENCE"))
            > observed_at
        || observed_at > record_time(&validation, &validation_name)
    {
        fail("PRE_REVIEW_CAUSAL_ORDER_MISMATCH");
    }
}

fn verify_review_admission(root: &Path, module_id: &str) {
    assert_project_root_safe(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    let module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("UNKNOWN_MODULE:{}", module_id)));
    let artifact = read_module_artifact(root, &project, module_id);
    module_artifact_matches_project(module, &artifact);
    explain_review_admission_preflight(root, module_id);
    assert_pre_review_validation_gate(root, module_id, &artifact);
    verify_internal(root, true, false);
    println!(
        "{{\"ok\":true,\"gate\":\"review_admission\",\"module_id\":\"{}\"}}",
        module_id
    );
}

fn explain_review_admission_preflight(root: &Path, module_id: &str) {
    let records_root = root.join(".appsdk").join("records");
    let evidence_root = records_root.join("evidence").join(module_id);
    let required = [
        (
            "fix_candidate",
            module_record_name("fix-candidate-record", module_id),
            "project::lifecycle_adapter",
            "produce from the clean owner worktree and candidate commit",
        ),
        (
            "development_whitebox",
            "evidence/<module>/whitebox-1.json".to_string(),
            "project::whitebox_adapter",
            "run the declared development whitebox and persist its actual result",
        ),
        (
            "deployment_install",
            "evidence/<module>/install-1.json".to_string(),
            "project::deployment_adapter",
            "install the exact candidate artifact and persist the real receipt",
        ),
        (
            "deployment_restart",
            "evidence/<module>/restart-1.json".to_string(),
            "project::deployment_adapter",
            "restart the exact installed artifact and persist the real receipt",
        ),
        (
            "deployed_blackbox",
            "evidence/<module>/blackbox-1.json".to_string(),
            "project::blackbox_adapter",
            "exercise the deployed public entrypoint and persist the actual result",
        ),
        (
            "pre_review_validation",
            module_record_name("pre-review-validation-record", module_id),
            "project::lifecycle_adapter",
            "bind the disjoint evidence IDs and causal timestamps after all gates pass",
        ),
    ];
    let missing: Vec<Value> = required
        .iter()
        .filter(|(_, relative, _, _)| {
            let path = if relative.starts_with("evidence/") {
                evidence_root.join(relative.strip_prefix("evidence/<module>/").unwrap())
            } else {
                records_root.join(relative)
            };
            !path.is_file()
        })
        .map(|(kind, relative, producer, next)| {
            serde_json::json!({
                "kind": kind,
                "path": relative,
                "producer": producer,
                "next": next
            })
        })
        .collect();
    if missing.is_empty()
        || !missing.iter().any(|entry| {
            matches!(
                entry.get("kind").and_then(Value::as_str),
                Some("fix_candidate" | "pre_review_validation")
            )
        })
    {
        return;
    }
    let present: Vec<String> = required
        .iter()
        .filter_map(|(_, relative, _, _)| {
            let path = if relative.starts_with("evidence/") {
                evidence_root.join(relative.strip_prefix("evidence/<module>/").unwrap())
            } else {
                records_root.join(relative)
            };
            path.is_file().then(|| relative.clone())
        })
        .collect();
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "error": "REVIEW_ADMISSION_BLOCKED",
            "module_id": module_id,
            "admission": "blocked",
            "missing": missing,
            "present": present,
            "retry_allowed": false,
            "idempotent": true,
            "next": "enable or run the declared project adapters; let each adapter persist real evidence; rerun the same admission command",
            "forbidden": [
                "do not hand-create lifecycle records",
                "do not copy records from another project or version",
                "do not invent hashes, receipts, timestamps, or producer identities",
                "do not retry this command until the listed external state changes"
            ]
        }))
        .unwrap()
    );
    std::process::exit(1);
}

fn assert_review_map_bindings(root: &Path, module_id: &str, review: &Value, review_name: &str) {
    let bindings = [
        ("resource-map.json", "/resource_map_hash"),
        ("function-map.json", "/function_map_hash"),
        ("mainline-call-map.json", "/mainline_call_map_hash"),
        ("verification-map.json", "/verification_map_hash"),
    ];
    if bindings.iter().all(|(map, path)| {
        record_str(review, path, review_name)
            == file_sha256(&root.join(".appsdk/maps").join(map), map)
    }) {
        return;
    }
    let project = read_project(root);
    let module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    let stage = module
        .get("stage")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT"));
    let migration =
        assert_sdk_migration_record(root).unwrap_or_else(|| fail("ARCHITECTURE_REVIEW_MAP_STALE"));
    let review_id = record_str(review, "/review_id", review_name);
    let retained_review = |key: &str| {
        migration
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|reviews| {
                reviews.iter().any(|entry| {
                    entry.get("module_id").and_then(Value::as_str) == Some(module_id)
                        && entry.get("review_id").and_then(Value::as_str) == Some(review_id)
                })
            })
    };
    if record_time(review, review_name) > record_time(&migration, "sdk-migration-record")
        || (!retained_review("frozen_reviews") && !retained_review("legacy_reconciled_reviews"))
        || (!matches!(stage, "frozen" | "retired") && !retained_review("legacy_reconciled_reviews"))
    {
        fail("ARCHITECTURE_REVIEW_MAP_STALE");
    }
    for (map, path) in bindings {
        let expected = record_str(review, path, review_name);
        let migration_entry = migration
            .get("maps")
            .and_then(Value::as_array)
            .and_then(|maps| {
                maps.iter()
                    .find(|entry| entry.get("name").and_then(Value::as_str) == Some(map))
            })
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        if expected != record_str(migration_entry, "/source_digest", "sdk-map-migration") {
            fail("ARCHITECTURE_REVIEW_MAP_STALE");
        }
    }
}

fn assert_fix_architecture_gate(root: &Path, module_id: &str, artifact: &Value) {
    let worktree_name = module_record_name("worktree-record", module_id);
    let reproduction_name = module_record_name("reproduction-record", module_id);
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let review_name = module_record_name("review-record", module_id);
    let worktree = read_record(root, &worktree_name);
    let reproduction = read_record(root, &reproduction_name);
    let candidate = read_record(root, &candidate_name);
    let review = read_record(root, &review_name);
    assert_pre_review_validation_gate(root, module_id, artifact);
    let validation_name = module_record_name("pre-review-validation-record", module_id);
    let validation = read_record(root, &validation_name);
    if record_str(&review, "/pre_review_validation_id", &review_name)
        != record_str(&validation, "/validation_id", &validation_name)
        || record_time(&validation, &validation_name) > record_time(&review, &review_name)
    {
        fail("PRE_REVIEW_VALIDATION_MISMATCH");
    }
    let issue_id = record_str(&worktree, "/issue_id", &worktree_name);
    let scope_hash = record_str(&worktree, "/scope_hash", &worktree_name);
    let candidate_commit = record_str(&candidate, "/head_commit", &candidate_name);
    let candidate_tree = record_str(&candidate, "/tree_hash", &candidate_name);
    if record_str(&worktree, "/module_id", &worktree_name) != module_id
        || record_str(&reproduction, "/module_id", &reproduction_name) != module_id
        || record_str(&candidate, "/module_id", &candidate_name) != module_id
        || record_str(&reproduction, "/issue_id", &reproduction_name) != issue_id
        || record_str(&candidate, "/issue_id", &candidate_name) != issue_id
        || record_str(&review, "/issue_id", &review_name) != issue_id
    {
        fail("FIX_ARCHITECTURE_SCOPE_MISMATCH");
    }
    if worktree.get("initial_clean") != Some(&Value::Bool(true))
        || worktree.get("final_clean") != Some(&Value::Bool(true))
        || worktree.get("isolation_mode").and_then(Value::as_str) != Some("isolated_worktree")
    {
        fail("FIX_WORKTREE_NOT_CLEAN_ISOLATED");
    }
    if record_str(&reproduction, "/worktree_id", &reproduction_name)
        != record_str(&worktree, "/worktree_id", &worktree_name)
        || record_str(&candidate, "/worktree_id", &candidate_name)
            != record_str(&worktree, "/worktree_id", &worktree_name)
        || record_str(&reproduction, "/base_commit", &reproduction_name)
            != record_str(&worktree, "/base_commit", &worktree_name)
        || record_str(&candidate, "/base_commit", &candidate_name)
            != record_str(&worktree, "/base_commit", &worktree_name)
        || reproduction.get("result").and_then(Value::as_str) != Some("reproduced")
    {
        fail("FIX_REPRODUCTION_GRAPH_MISMATCH");
    }
    if record_str(&worktree, "/head_commit", &worktree_name) != candidate_commit
        || record_str(&candidate, "/scope_hash", &candidate_name) != scope_hash
        || record_str(&review, "/review_kind", &review_name) != "architecture"
        || record_str(&review, "/fix_candidate_id", &review_name)
            != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(&review, "/reviewed_commit", &review_name) != candidate_commit
        || record_str(&review, "/reviewed_tree_hash", &review_name) != candidate_tree
        || record_str(&review, "/reviewed_diff_hash", &review_name)
            != record_str(&candidate, "/diff_hash", &candidate_name)
        || record_str(&review, "/reviewed_scope_hash", &review_name) != scope_hash
        || record_str(&review, "/reviewed_artifact_hash", &review_name)
            != record_str(artifact, "/artifact_hash", "artifact")
        || review.get("verdict").and_then(Value::as_str) != Some("pass")
    {
        fail("ARCHITECTURE_REVIEW_INPUT_MISMATCH");
    }
    assert_review_map_bindings(root, module_id, &review, &review_name);
    let confidence = review
        .get("ai_confidence")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| fail("INVALID_REVIEW_CONFIDENCE"));
    if !(0.0..=1.0).contains(&confidence)
        || record_str(&review, "/confidence_rationale", &review_name).is_empty()
    {
        fail("INVALID_REVIEW_CONFIDENCE");
    }
    let baseline_id = record_str(&reproduction, "/baseline_evidence_id", &reproduction_name);
    let baseline = evidence_by_id(root, module_id, baseline_id);
    assert_evidence_record(&baseline, baseline_id, Utc::now());
    if record_str(&baseline, "/phase", baseline_id) != "baseline_reproduction"
        || baseline.get("result").and_then(Value::as_str) != Some("pass")
        || baseline.get("input_hashes") != reproduction.get("input_hashes")
    {
        fail("BASELINE_REPRODUCTION_EVIDENCE_MISMATCH");
    }
    let mut candidate_phases = Vec::new();
    for value in record_array(&candidate, "/verification_evidence_ids", &candidate_name) {
        let id = value
            .as_str()
            .unwrap_or_else(|| fail("INVALID_CANDIDATE_EVIDENCE_ID"));
        let evidence = evidence_by_id(root, module_id, id);
        assert_evidence_record(&evidence, id, Utc::now());
        if record_str(&evidence, "/issue_id", id) != issue_id
            || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
            || record_str(&evidence, "/scope_hash", id) != scope_hash
            || record_str(&evidence, "/source_commit", id) != candidate_commit
            || evidence.get("result").and_then(Value::as_str) != Some("pass")
            || record_time(&evidence, id) > record_time(&review, &review_name)
        {
            fail("FIX_CANDIDATE_EVIDENCE_MISMATCH");
        }
        candidate_phases.push(record_str(&evidence, "/phase", id).to_string());
    }
    for phase in [
        "fix_candidate",
        "positive_intervention",
        "negative_intervention",
    ] {
        if !candidate_phases.iter().any(|value| value == phase) {
            fail(format!("MISSING_FIX_EVIDENCE_PHASE:{}", phase));
        }
    }
    for value in record_array(&review, "/evidence_ids", &review_name) {
        let id = value
            .as_str()
            .unwrap_or_else(|| fail("INVALID_REVIEW_EVIDENCE_ID"));
        let evidence = evidence_by_id(root, module_id, id);
        assert_evidence_record(&evidence, id, Utc::now());
        if record_str(&evidence, "/issue_id", id) != issue_id
            || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
            || record_str(&evidence, "/scope_hash", id) != scope_hash
            || record_str(&evidence, "/source_commit", id) != candidate_commit
            || evidence.get("result").and_then(Value::as_str) != Some("pass")
            || record_str(&evidence, "/phase", id) == "post_architecture_effectiveness"
            || record_time(&evidence, id) > record_time(&review, &review_name)
        {
            fail("ARCHITECTURE_REVIEW_EVIDENCE_MISMATCH");
        }
    }
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "FIX_CANDIDATE_COMMIT_MISSING",
    ) != candidate_tree
        || !(record_time(&worktree, &worktree_name)
            <= record_time(&reproduction, &reproduction_name)
            && record_time(&reproduction, &reproduction_name)
                <= record_time(&candidate, &candidate_name)
            && record_time(&candidate, &candidate_name) <= record_time(&review, &review_name))
    {
        fail("FIX_ARCHITECTURE_ORDER_OR_IDENTITY_INVALID");
    }
}

fn assert_fix_effectiveness_gate(root: &Path, module_id: &str) {
    let reproduction_name = module_record_name("reproduction-record", module_id);
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let review_name = module_record_name("review-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let reproduction = read_record(root, &reproduction_name);
    let candidate = read_record(root, &candidate_name);
    let review = read_record(root, &review_name);
    let effectiveness = read_record(root, &effectiveness_name);
    let issue_id = record_str(&candidate, "/issue_id", &candidate_name);
    let scope_hash = record_str(&candidate, "/scope_hash", &candidate_name);
    let candidate_commit = record_str(&candidate, "/head_commit", &candidate_name);
    let candidate_tree = record_str(&candidate, "/tree_hash", &candidate_name);
    if record_str(&effectiveness, "/issue_id", &effectiveness_name) != issue_id
        || record_str(&effectiveness, "/module_id", &effectiveness_name) != module_id
        || record_str(&effectiveness, "/fix_candidate_id", &effectiveness_name)
            != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(
            &effectiveness,
            "/architecture_review_id",
            &effectiveness_name,
        ) != record_str(&review, "/review_id", &review_name)
        || record_str(&effectiveness, "/reviewed_commit", &effectiveness_name) != candidate_commit
        || record_str(&effectiveness, "/reviewed_tree_hash", &effectiveness_name) != candidate_tree
        || effectiveness.get("reproduction_input_hashes") != reproduction.get("input_hashes")
        || effectiveness.get("source_unchanged_since_review") != Some(&Value::Bool(true))
        || effectiveness.get("result").and_then(Value::as_str) != Some("pass")
        || record_time(&review, &review_name) > record_time(&effectiveness, &effectiveness_name)
    {
        fail("POST_ARCHITECTURE_EFFECTIVENESS_MISMATCH");
    }
    let baseline_id = record_str(&reproduction, "/baseline_evidence_id", &reproduction_name);
    if record_str(&effectiveness, "/baseline_evidence_id", &effectiveness_name) != baseline_id {
        fail("POST_ARCHITECTURE_BASELINE_MISMATCH");
    }
    let mut ids = vec![record_str(
        &effectiveness,
        "/fixed_replay_evidence_id",
        &effectiveness_name,
    )
    .to_string()];
    for path in [
        "/positive_evidence_ids",
        "/negative_evidence_ids",
        "/blackbox_evidence_ids",
    ] {
        ids.extend(
            record_array(&effectiveness, path, &effectiveness_name)
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| fail("INVALID_EFFECTIVENESS_EVIDENCE_ID"))
                        .to_string()
                }),
        );
    }
    ids.sort();
    ids.dedup();
    let mut phases = Vec::new();
    for id in ids {
        let evidence = evidence_by_id(root, module_id, &id);
        assert_evidence_record(&evidence, &id, Utc::now());
        if record_str(&evidence, "/issue_id", &id) != issue_id
            || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
            || record_str(&evidence, "/scope_hash", &id) != scope_hash
            || record_str(&evidence, "/source_commit", &id) != candidate_commit
            || evidence.get("result").and_then(Value::as_str) != Some("pass")
            || record_time(&evidence, &id) < record_time(&review, &review_name)
        {
            fail("POST_ARCHITECTURE_EFFECTIVENESS_EVIDENCE_MISMATCH");
        }
        phases.push(record_str(&evidence, "/phase", &id).to_string());
    }
    for phase in [
        "positive_intervention",
        "negative_intervention",
        "post_architecture_effectiveness",
    ] {
        if !phases.iter().any(|value| value == phase) {
            fail(format!("MISSING_EFFECTIVENESS_EVIDENCE_PHASE:{}", phase));
        }
    }
}

fn assert_parallel_merge_gate(root: &Path, module_id: &str) {
    let worktree_name = module_record_name("worktree-record", module_id);
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let promotion_name = module_record_name("promotion-record", module_id);
    let merge_name = module_record_name("merge-record", module_id);
    let worktree = read_record(root, &worktree_name);
    let candidate = read_record(root, &candidate_name);
    let effectiveness = read_record(root, &effectiveness_name);
    let promotion = read_record(root, &promotion_name);
    let collaboration_name = format!(
        "collaboration-record-{}.json",
        record_str(&promotion, "/collaboration_record_id", &promotion_name)
    );
    let queue_name = format!(
        "merge-queue-record-{}.json",
        record_str(&promotion, "/merge_queue_record_id", &promotion_name)
    );
    let integration_name = format!(
        "integration-record-{}.json",
        record_str(&promotion, "/integration_record_id", &promotion_name)
    );
    let receipt_name = format!(
        "mainline-receipt-record-{}.json",
        record_str(&promotion, "/mainline_receipt_record_id", &promotion_name)
    );
    let collaboration = read_record(root, &collaboration_name);
    let queue = read_record(root, &queue_name);
    let integration = read_record(root, &integration_name);
    let receipt = read_record(root, &receipt_name);
    let collaboration_index = read_record(root, "collaboration-index.json");
    let queue_state = read_record(root, "merge-queue-state.json");
    let merge = read_record(root, &merge_name);
    let issue_id = record_str(&candidate, "/issue_id", &candidate_name);
    let candidate_id = record_str(&candidate, "/fix_candidate_id", &candidate_name);
    let candidate_commit = record_str(&candidate, "/head_commit", &candidate_name);
    let candidate_tree = record_str(&candidate, "/tree_hash", &candidate_name);
    let effectiveness_id = record_str(&effectiveness, "/effectiveness_id", &effectiveness_name);
    let collaboration_id = record_str(&collaboration, "/collaboration_id", &collaboration_name);
    let queue_id = record_str(&queue, "/queue_entry_id", &queue_name);
    let integration_id = record_str(&integration, "/integration_id", &integration_name);
    let receipt_id = record_str(&receipt, "/receipt_id", &receipt_name);
    let milestone_id = record_str(&collaboration, "/milestone_id", &collaboration_name);
    let parent_task_id = record_str(&collaboration, "/parent_task_id", &collaboration_name);
    let milestone_sequence = collaboration
        .get("milestone_sequence")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let integration_commit = record_str(&integration, "/integration_commit", &integration_name);
    let integration_tree = record_str(&integration, "/integration_tree_hash", &integration_name);
    let main_base_commit = record_str(&queue, "/main_base_commit", &queue_name);

    if milestone_id.is_empty()
        || parent_task_id.is_empty()
        || milestone_sequence == 0
        || record_str(&collaboration, "/milestone_scope", &collaboration_name).is_empty()
        || collaboration.get("independently_verifiable") != Some(&Value::Bool(true))
        || collaboration.get("one_milestone_per_worktree") != Some(&Value::Bool(true))
        || record_str(&worktree, "/milestone_id", &worktree_name) != milestone_id
    {
        fail("INCREMENTAL_MILESTONE_CONTRACT_REQUIRED");
    }
    let predecessor_collaboration_id = record_str(
        &collaboration,
        "/predecessor_collaboration_id",
        &collaboration_name,
    );
    let predecessor_receipt_id = record_str(
        &collaboration,
        "/predecessor_receipt_id",
        &collaboration_name,
    );
    if milestone_sequence == 1 {
        if predecessor_collaboration_id != "none" || predecessor_receipt_id != "none" {
            fail("FIRST_MILESTONE_PREDECESSOR_INVALID");
        }
    } else {
        if predecessor_collaboration_id == "none" || predecessor_receipt_id == "none" {
            fail("MILESTONE_PREDECESSOR_RECEIPT_REQUIRED");
        }
        let predecessor_collaboration_name =
            format!("collaboration-record-{}.json", predecessor_collaboration_id);
        let predecessor_receipt_name =
            format!("mainline-receipt-record-{}.json", predecessor_receipt_id);
        let predecessor_collaboration = read_record(root, &predecessor_collaboration_name);
        let predecessor_receipt = read_record(root, &predecessor_receipt_name);
        if record_str(
            &predecessor_collaboration,
            "/parent_task_id",
            &predecessor_collaboration_name,
        ) != parent_task_id
            || predecessor_collaboration
                .get("milestone_sequence")
                .and_then(Value::as_u64)
                != Some(milestone_sequence - 1)
            || record_str(
                &predecessor_collaboration,
                "/worktree_id",
                &predecessor_collaboration_name,
            ) == record_str(&collaboration, "/worktree_id", &collaboration_name)
            || predecessor_receipt.get("remote_verified") != Some(&Value::Bool(true))
            || predecessor_receipt.get("result").and_then(Value::as_str) != Some("pass")
        {
            fail("MILESTONE_PREDECESSOR_MISMATCH");
        }
        let predecessor_remote_commit = record_str(
            &predecessor_receipt,
            "/remote_main_commit",
            &predecessor_receipt_name,
        );
        let current_base = record_str(&worktree, "/base_commit", &worktree_name);
        let inherited = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "merge-base",
                "--is-ancestor",
                predecessor_remote_commit,
                current_base,
            ])
            .status()
            .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
        if !inherited.success() {
            fail("NEXT_MILESTONE_BASE_PRECEDES_REMOTE_RECEIPT");
        }
    }

    for (record, name) in [
        (&collaboration, collaboration_name.as_str()),
        (&queue, queue_name.as_str()),
        (&integration, integration_name.as_str()),
        (&receipt, receipt_name.as_str()),
        (&merge, merge_name.as_str()),
    ] {
        if record_str(record, "/issue_id", name) != issue_id
            || record_str(record, "/module_id", name) != module_id
        {
            fail("PARALLEL_DEVELOPMENT_SCOPE_MISMATCH");
        }
    }
    if collaboration.get("scenario_ids")
        != Some(&serde_json::json!([
            "multi_worker_collaboration",
            "multi_worktree_merge_queue"
        ]))
        || record_str(&collaboration, "/worktree_id", &collaboration_name)
            != record_str(&worktree, "/worktree_id", &worktree_name)
        || collaboration.get("exclusive_worktree") != Some(&Value::Bool(true))
        || collaboration.get("exclusive_claim") != Some(&Value::Bool(true))
        || collaboration.get("status").and_then(Value::as_str) != Some("handoff_ready")
    {
        fail("MULTI_WORKER_EXCLUSIVE_WORKTREE_REQUIRED");
    }
    for path in ["/run_id", "/semantic_claim_id", "/worker_id"] {
        if record_str(&collaboration, path, &collaboration_name).is_empty() {
            fail("INVALID_COLLABORATION_IDENTITY");
        }
    }
    let active_claims = record_array(
        &collaboration_index,
        "/active_claims",
        "collaboration-index.json",
    );
    let mut claim_ids = std::collections::HashSet::new();
    let mut worker_ids = std::collections::HashSet::new();
    let mut worktree_ids = std::collections::HashSet::new();
    let mut milestone_ids = std::collections::HashSet::new();
    let mut current_claim_found = false;
    for claim in active_claims {
        let semantic_id = record_str(claim, "/semantic_claim_id", "collaboration-index.json");
        let worker_id = record_str(claim, "/worker_id", "collaboration-index.json");
        let worktree_id = record_str(claim, "/worktree_id", "collaboration-index.json");
        let indexed_milestone_id = record_str(claim, "/milestone_id", "collaboration-index.json");
        if !claim_ids.insert(semantic_id)
            || !worker_ids.insert(worker_id)
            || !worktree_ids.insert(worktree_id)
            || !milestone_ids.insert(indexed_milestone_id)
        {
            fail("COLLABORATION_INDEX_NOT_EXCLUSIVE");
        }
        if record_str(claim, "/collaboration_id", "collaboration-index.json") == collaboration_id
            && indexed_milestone_id == milestone_id
        {
            current_claim_found = true;
        }
    }
    if !current_claim_found {
        fail("COLLABORATION_NOT_ACTIVE");
    }
    if record_str(&queue, "/collaboration_id", &queue_name) != collaboration_id
        || record_str(&queue, "/milestone_id", &queue_name) != milestone_id
        || queue.get("delivery_mode").and_then(Value::as_str) != Some("commit_merge_each_milestone")
        || record_str(&queue, "/fix_candidate_id", &queue_name) != candidate_id
        || record_str(&queue, "/effectiveness_id", &queue_name) != effectiveness_id
        || record_str(&queue, "/candidate_commit", &queue_name) != candidate_commit
        || queue
            .get("queue_position")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            == 0
        || record_str(&queue, "/merge_owner", &queue_name).is_empty()
        || queue.get("strategy").and_then(Value::as_str)
            != Some("integration_merge_then_fast_forward")
        || queue.get("status").and_then(Value::as_str) != Some("admitted")
    {
        fail("MERGE_QUEUE_ADMISSION_MISMATCH");
    }
    let ordered_entries =
        record_array(&queue_state, "/ordered_entry_ids", "merge-queue-state.json");
    let mut unique_entries = std::collections::HashSet::new();
    if record_str(&queue_state, "/merge_owner", "merge-queue-state.json")
        != record_str(&queue, "/merge_owner", &queue_name)
        || record_str(&queue_state, "/active_entry_id", "merge-queue-state.json") != queue_id
        || ordered_entries
            .iter()
            .any(|entry| !unique_entries.insert(entry.as_str().unwrap_or("")))
        || ordered_entries
            .get(
                queue
                    .get("queue_position")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize
                    - 1,
            )
            .and_then(Value::as_str)
            != Some(queue_id)
    {
        fail("GLOBAL_MERGE_QUEUE_STATE_MISMATCH");
    }
    let gate_results = integration
        .get("required_gate_results")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INTEGRATION_GATES_MISSING"));
    let verification_map: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".appsdk/maps/verification-map.json"))
            .unwrap_or_else(|_| fail("VERIFICATION_MAP_MISSING")),
    )
    .unwrap_or_else(|_| fail("INVALID_VERIFICATION_MAP"));
    let expected_gates = record_array(&verification_map, "/gates", "verification-map.json")
        .iter()
        .filter(|gate| {
            gate.get("required_for")
                .and_then(Value::as_array)
                .is_some_and(|uses| {
                    uses.iter()
                        .any(|value| value.as_str() == Some("integration_verification"))
                })
        })
        .map(|gate| {
            (
                record_str(gate, "/gate_id", "verification-map.json"),
                record_str(gate, "/producer", "verification-map.json"),
            )
        })
        .collect::<Vec<_>>();
    let actual_gates = gate_results
        .iter()
        .map(|gate| {
            if gate.get("result").and_then(Value::as_str) != Some("pass")
                || record_str(gate, "/source_commit", &integration_name) != integration_commit
                || record_str(gate, "/tree_hash", &integration_name) != integration_tree
            {
                fail("INTEGRATION_GATE_BINDING_MISMATCH");
            }
            (
                record_str(gate, "/gate_id", &integration_name),
                record_str(gate, "/producer", &integration_name),
            )
        })
        .collect::<Vec<_>>();
    if expected_gates.is_empty()
        || actual_gates.len() != expected_gates.len()
        || actual_gates
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            != actual_gates.len()
        || actual_gates
            .iter()
            .any(|gate| !expected_gates.contains(gate))
        || record_str(&integration, "/queue_entry_id", &integration_name) != queue_id
        || record_str(&integration, "/milestone_id", &integration_name) != milestone_id
        || record_str(&integration, "/candidate_commit", &integration_name) != candidate_commit
        || record_str(&integration, "/main_base_commit", &integration_name) != main_base_commit
        || integration.get("conflict_status").and_then(Value::as_str) != Some("clean")
        || integration.get("resolution_mode").and_then(Value::as_str) != Some("none")
        || !matches!(
            integration.get("impact_status").and_then(Value::as_str),
            Some("unchanged" | "revalidated")
        )
        || integration.get("result").and_then(Value::as_str) != Some("pass")
    {
        fail("INTEGRATION_RECORD_MISMATCH");
    }
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "FIX_CANDIDATE_COMMIT_MISSING",
    ) != candidate_tree
        || git_value(
            root,
            &["rev-parse", &format!("{}^{{tree}}", integration_commit)],
            "INTEGRATION_COMMIT_MISSING",
        ) != integration_tree
    {
        fail("TESTED_INTEGRATION_TREE_MISMATCH");
    }
    for ancestor in [candidate_commit, main_base_commit] {
        let reachable = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["merge-base", "--is-ancestor", ancestor, integration_commit])
            .status()
            .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
        if !reachable.success() {
            fail("INTEGRATION_ANCESTRY_MISMATCH");
        }
    }
    let local_main_ref = record_str(&receipt, "/local_main_ref", &receipt_name);
    let remote_name = record_str(&receipt, "/remote_name", &receipt_name);
    let remote_ref = record_str(&receipt, "/remote_ref", &receipt_name);
    let local_main_commit = git_value(
        root,
        &["rev-parse", local_main_ref],
        "LOCAL_MAIN_REF_MISSING",
    );
    let remote_main_commit = git_ls_remote(root, remote_name, remote_ref);
    if record_str(&receipt, "/integration_id", &receipt_name) != integration_id
        || record_str(&receipt, "/queue_entry_id", &receipt_name) != queue_id
        || record_str(&receipt, "/milestone_id", &receipt_name) != milestone_id
        || record_str(&receipt, "/integration_commit", &receipt_name) != integration_commit
        || record_str(&receipt, "/local_main_commit", &receipt_name) != local_main_commit
        || record_str(&receipt, "/remote_main_commit", &receipt_name) != remote_main_commit
        || record_str(&receipt, "/integration_tree_hash", &receipt_name) != integration_tree
        || receipt.get("candidate_reachable") != Some(&Value::Bool(true))
        || receipt.get("integration_local_reachable") != Some(&Value::Bool(true))
        || receipt.get("integration_remote_reachable") != Some(&Value::Bool(true))
        || receipt.get("remote_verified") != Some(&Value::Bool(true))
        || receipt.get("result").and_then(Value::as_str) != Some("pass")
    {
        fail("MAINLINE_RECEIPT_MISMATCH");
    }
    for main_commit in [&local_main_commit, &remote_main_commit] {
        let reachable = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "merge-base",
                "--is-ancestor",
                integration_commit,
                main_commit,
            ])
            .status()
            .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
        if !reachable.success() {
            fail("INTEGRATION_NOT_REACHABLE_FROM_MAIN");
        }
    }
    if record_str(&merge, "/queue_entry_id", &merge_name) != queue_id
        || record_str(&merge, "/integration_id", &merge_name) != integration_id
        || record_str(&merge, "/mainline_receipt_id", &merge_name) != receipt_id
        || record_str(&merge, "/milestone_id", &merge_name) != milestone_id
        || record_str(&merge, "/fix_candidate_id", &merge_name) != candidate_id
        || record_str(&merge, "/effectiveness_id", &merge_name) != effectiveness_id
        || record_str(&merge, "/mainline_ref", &merge_name) != local_main_ref
        || record_str(&merge, "/candidate_commit", &merge_name) != candidate_commit
        || record_str(&merge, "/integration_commit", &merge_name) != integration_commit
        || record_str(&merge, "/merge_commit", &merge_name) != integration_commit
        || record_str(&merge, "/candidate_tree_hash", &merge_name) != candidate_tree
        || record_str(&merge, "/integration_tree_hash", &merge_name) != integration_tree
        || record_str(&merge, "/merged_tree_hash", &merge_name) != integration_tree
        || merge.get("change_identity").and_then(Value::as_str) != Some("tested_integration_exact")
        || merge.get("result").and_then(Value::as_str) != Some("pass")
    {
        fail("PARALLEL_MAINLINE_MERGE_MISMATCH");
    }
    if !(record_time(&collaboration, &collaboration_name) <= record_time(&queue, &queue_name)
        && record_time(&effectiveness, &effectiveness_name) <= record_time(&queue, &queue_name)
        && record_time(&queue, &queue_name) <= record_time(&integration, &integration_name)
        && record_time(&integration, &integration_name) <= record_time(&receipt, &receipt_name)
        && record_time(&receipt, &receipt_name) <= record_time(&merge, &merge_name))
    {
        fail("PARALLEL_MERGE_ORDER_INVALID");
    }
}

fn resolve_recorded_mainline_commit(root: &Path, mainline_ref: &str) -> String {
    let exact = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{}^{{commit}}", mainline_ref),
        ])
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if exact.status.success() {
        let commit = String::from_utf8_lossy(&exact.stdout).trim().to_string();
        if commit.is_empty() {
            fail("MAINLINE_REF_MISSING");
        }
        return commit;
    }

    let branch = mainline_ref
        .strip_prefix("refs/heads/")
        .unwrap_or(mainline_ref);
    let valid = Command::new("git")
        .args(["check-ref-format", "--branch", branch])
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !valid.status.success() {
        fail("MAINLINE_REF_MISSING");
    }
    let refs = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["for-each-ref", "--format=%(refname)", "refs/remotes"])
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !refs.status.success() {
        fail("VCS_ADAPTER_FAILED");
    }
    let suffix = format!("/{}", branch);
    let refs_text = String::from_utf8_lossy(&refs.stdout);
    let matches = refs_text
        .lines()
        .filter(|reference| reference.ends_with(&suffix))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [reference] => git_value(
            root,
            &["rev-parse", &format!("{}^{{commit}}", reference)],
            "MAINLINE_REF_MISSING",
        ),
        [] => fail("MAINLINE_REF_MISSING"),
        _ => fail("MAINLINE_REF_AMBIGUOUS"),
    }
}

fn assert_single_merge_gate(root: &Path, module_id: &str) {
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let merge_name = module_record_name("merge-record", module_id);
    let candidate = read_record(root, &candidate_name);
    let effectiveness = read_record(root, &effectiveness_name);
    let merge = read_record(root, &merge_name);
    let candidate_commit = record_str(&candidate, "/head_commit", &candidate_name);
    let candidate_tree = record_str(&candidate, "/tree_hash", &candidate_name);
    let merge_commit = record_str(&merge, "/merge_commit", &merge_name);
    if record_str(&merge, "/issue_id", &merge_name)
        != record_str(&candidate, "/issue_id", &candidate_name)
        || record_str(&merge, "/module_id", &merge_name) != module_id
        || record_str(&merge, "/fix_candidate_id", &merge_name)
            != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(&merge, "/effectiveness_id", &merge_name)
            != record_str(&effectiveness, "/effectiveness_id", &effectiveness_name)
        || record_str(&merge, "/candidate_commit", &merge_name) != candidate_commit
        || record_str(&merge, "/candidate_tree_hash", &merge_name) != candidate_tree
        || record_str(&merge, "/merged_tree_hash", &merge_name) != candidate_tree
        || merge.get("change_identity").and_then(Value::as_str) != Some("exact")
        || merge.get("result").and_then(Value::as_str) != Some("pass")
        || record_time(&effectiveness, &effectiveness_name) > record_time(&merge, &merge_name)
    {
        fail("MAINLINE_MERGE_RECORD_MISMATCH");
    }
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "FIX_CANDIDATE_COMMIT_MISSING",
    ) != candidate_tree
        || git_value(
            root,
            &["rev-parse", &format!("{}^{{tree}}", merge_commit)],
            "MAINLINE_MERGE_COMMIT_MISSING",
        ) != candidate_tree
    {
        fail("MAINLINE_MERGE_IDENTITY_MISMATCH");
    }
    let candidate_merged = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "merge-base",
            "--is-ancestor",
            candidate_commit,
            merge_commit,
        ])
        .status()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !candidate_merged.success() {
        fail("FIX_CANDIDATE_NOT_MERGED");
    }
    let mainline_head =
        resolve_recorded_mainline_commit(root, record_str(&merge, "/mainline_ref", &merge_name));
    let merge_on_mainline = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["merge-base", "--is-ancestor", merge_commit, &mainline_head])
        .status()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !merge_on_mainline.success() {
        fail("RECORDED_MERGE_NOT_ON_MAINLINE");
    }
}

fn assert_fix_merge_gate(root: &Path, module_id: &str) {
    let project = read_project(root);
    if assert_development_scenarios(root, &project) {
        assert_parallel_merge_gate(root, module_id);
    } else {
        assert_single_merge_gate(root, module_id);
    }
}

fn assert_fix_lifecycle_graph(
    root: &Path,
    module_id: &str,
    review: &Value,
    promotion: &Value,
    artifact: &Value,
) {
    let project = read_project(root);
    let parallel_development = assert_development_scenarios(root, &project);
    assert_fix_architecture_gate(root, module_id, artifact);
    assert_fix_effectiveness_gate(root, module_id);
    assert_fix_merge_gate(root, module_id);
    let worktree_name = module_record_name("worktree-record", module_id);
    let reproduction_name = module_record_name("reproduction-record", module_id);
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let effectiveness_name = module_record_name("effectiveness-record", module_id);
    let merge_name = module_record_name("merge-record", module_id);
    let worktree = read_record(root, &worktree_name);
    let reproduction = read_record(root, &reproduction_name);
    let candidate = read_record(root, &candidate_name);
    let effectiveness = read_record(root, &effectiveness_name);
    let merge = read_record(root, &merge_name);

    let issue_id = record_str(&worktree, "/issue_id", &worktree_name);
    let scope_hash = record_str(&worktree, "/scope_hash", &worktree_name);
    let base_commit = record_str(&worktree, "/base_commit", &worktree_name);
    let candidate_commit = record_str(&candidate, "/head_commit", &candidate_name);
    let candidate_tree = record_str(&candidate, "/tree_hash", &candidate_name);
    let review_id = record_str(review, "/review_id", "review-record.json");
    for (record, name) in [
        (&reproduction, reproduction_name.as_str()),
        (&candidate, candidate_name.as_str()),
        (&effectiveness, effectiveness_name.as_str()),
        (&merge, merge_name.as_str()),
        (review, "review-record.json"),
        (promotion, "promotion-record.json"),
    ] {
        if record_str(record, "/issue_id", name) != issue_id {
            fail("FIX_LIFECYCLE_ISSUE_MISMATCH");
        }
    }
    for (record, name) in [
        (&worktree, worktree_name.as_str()),
        (&reproduction, reproduction_name.as_str()),
        (&candidate, candidate_name.as_str()),
        (&effectiveness, effectiveness_name.as_str()),
        (&merge, merge_name.as_str()),
    ] {
        if record_str(record, "/module_id", name) != module_id {
            fail("FIX_LIFECYCLE_MODULE_MISMATCH");
        }
    }
    if worktree.get("initial_clean") != Some(&Value::Bool(true))
        || worktree.get("final_clean") != Some(&Value::Bool(true))
        || worktree.get("isolation_mode").and_then(Value::as_str) != Some("isolated_worktree")
    {
        fail("FIX_WORKTREE_NOT_CLEAN_ISOLATED");
    }
    if record_str(&reproduction, "/worktree_id", &reproduction_name)
        != record_str(&worktree, "/worktree_id", &worktree_name)
        || record_str(&candidate, "/worktree_id", &candidate_name)
            != record_str(&worktree, "/worktree_id", &worktree_name)
        || record_str(&reproduction, "/base_commit", &reproduction_name) != base_commit
        || record_str(&candidate, "/base_commit", &candidate_name) != base_commit
        || reproduction.get("result").and_then(Value::as_str) != Some("reproduced")
    {
        fail("FIX_REPRODUCTION_GRAPH_MISMATCH");
    }
    if record_str(&worktree, "/head_commit", &worktree_name) != candidate_commit
        || record_str(&candidate, "/scope_hash", &candidate_name) != scope_hash
        || record_str(review, "/review_kind", "review-record.json") != "architecture"
        || record_str(review, "/fix_candidate_id", "review-record.json")
            != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(review, "/reviewed_commit", "review-record.json") != candidate_commit
        || record_str(review, "/reviewed_tree_hash", "review-record.json") != candidate_tree
        || record_str(review, "/reviewed_scope_hash", "review-record.json") != scope_hash
        || record_str(review, "/reviewed_diff_hash", "review-record.json")
            != record_str(&candidate, "/diff_hash", &candidate_name)
        || review.get("verdict").and_then(Value::as_str) != Some("pass")
    {
        fail("ARCHITECTURE_REVIEW_INPUT_MISMATCH");
    }
    assert_review_map_bindings(root, module_id, review, "review-record.json");
    if record_str(&effectiveness, "/fix_candidate_id", &effectiveness_name)
        != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(
            &effectiveness,
            "/architecture_review_id",
            &effectiveness_name,
        ) != review_id
        || record_str(&effectiveness, "/reviewed_commit", &effectiveness_name) != candidate_commit
        || record_str(&effectiveness, "/reviewed_tree_hash", &effectiveness_name) != candidate_tree
        || effectiveness.get("source_unchanged_since_review") != Some(&Value::Bool(true))
        || effectiveness.get("result").and_then(Value::as_str) != Some("pass")
        || effectiveness.get("reproduction_input_hashes") != reproduction.get("input_hashes")
    {
        fail("POST_ARCHITECTURE_EFFECTIVENESS_MISMATCH");
    }
    let baseline_id = record_str(&reproduction, "/baseline_evidence_id", &reproduction_name);
    if record_str(&effectiveness, "/baseline_evidence_id", &effectiveness_name) != baseline_id {
        fail("POST_ARCHITECTURE_BASELINE_MISMATCH");
    }
    let mut required_evidence = vec![baseline_id.to_string()];
    required_evidence.push(
        record_str(
            &effectiveness,
            "/fixed_replay_evidence_id",
            &effectiveness_name,
        )
        .to_string(),
    );
    for path in [
        "/positive_evidence_ids",
        "/negative_evidence_ids",
        "/blackbox_evidence_ids",
    ] {
        required_evidence.extend(
            record_array(&effectiveness, path, &effectiveness_name)
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| fail("INVALID_EFFECTIVENESS_EVIDENCE_ID"))
                        .to_string()
                }),
        );
    }
    required_evidence.extend(
        record_array(&candidate, "/verification_evidence_ids", &candidate_name)
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .unwrap_or_else(|| fail("INVALID_CANDIDATE_EVIDENCE_ID"))
                    .to_string()
            }),
    );
    required_evidence.sort();
    required_evidence.dedup();
    let mut phases = Vec::new();
    for id in &required_evidence {
        let evidence = evidence_by_id(root, module_id, id);
        assert_evidence_record(&evidence, id, Utc::now());
        if record_str(&evidence, "/evidence_id", id) != id
            || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
            || record_str(&evidence, "/issue_id", id) != issue_id
            || record_str(&evidence, "/scope_hash", id) != scope_hash
            || evidence.get("result").and_then(Value::as_str) != Some("pass")
        {
            fail("FIX_EVIDENCE_SCOPE_MISMATCH");
        }
        phases.push(record_str(&evidence, "/phase", id).to_string());
    }
    for phase in [
        "baseline_reproduction",
        "fix_candidate",
        "positive_intervention",
        "negative_intervention",
        "post_architecture_effectiveness",
    ] {
        if !phases.iter().any(|value| value == phase) {
            fail(format!("MISSING_FIX_EVIDENCE_PHASE:{}", phase));
        }
    }
    for value in record_array(review, "/evidence_ids", "review-record.json") {
        let id = value
            .as_str()
            .unwrap_or_else(|| fail("INVALID_REVIEW_EVIDENCE_ID"));
        let evidence = evidence_by_id(root, module_id, id);
        assert_evidence_record(&evidence, id, Utc::now());
        if record_str(&evidence, "/phase", id) == "post_architecture_effectiveness"
            || record_time(&evidence, id) > record_time(review, "review-record.json")
        {
            fail("ARCHITECTURE_REVIEW_USES_POST_REVIEW_EVIDENCE");
        }
    }
    let merge_commit = record_str(&merge, "/merge_commit", &merge_name);
    if record_str(promotion, "/worktree_record_id", "promotion-record.json")
        != record_str(&worktree, "/worktree_id", &worktree_name)
        || record_str(
            promotion,
            "/reproduction_record_id",
            "promotion-record.json",
        ) != record_str(&reproduction, "/reproduction_id", &reproduction_name)
        || record_str(promotion, "/fix_candidate_id", "promotion-record.json")
            != record_str(&candidate, "/fix_candidate_id", &candidate_name)
        || record_str(
            promotion,
            "/architecture_review_id",
            "promotion-record.json",
        ) != review_id
        || record_str(
            promotion,
            "/effectiveness_record_id",
            "promotion-record.json",
        ) != record_str(&effectiveness, "/effectiveness_id", &effectiveness_name)
        || record_str(promotion, "/merge_record_id", "promotion-record.json")
            != record_str(&merge, "/merge_id", &merge_name)
        || record_str(promotion, "/candidate_commit", "promotion-record.json") != candidate_commit
        || record_str(promotion, "/merged_commit", "promotion-record.json") != merge_commit
        || record_str(promotion, "/source_commit", "promotion-record.json") != merge_commit
    {
        fail("PROMOTION_FIX_LIFECYCLE_REFERENCE_MISMATCH");
    }
    if parallel_development {
        let queue_name = format!(
            "merge-queue-record-{}.json",
            record_str(promotion, "/merge_queue_record_id", "promotion-record.json")
        );
        let integration_name = format!(
            "integration-record-{}.json",
            record_str(promotion, "/integration_record_id", "promotion-record.json")
        );
        let receipt_name = format!(
            "mainline-receipt-record-{}.json",
            record_str(
                promotion,
                "/mainline_receipt_record_id",
                "promotion-record.json",
            )
        );
        let queue = read_record(root, &queue_name);
        let integration = read_record(root, &integration_name);
        let receipt = read_record(root, &receipt_name);
        if record_str(promotion, "/merge_queue_record_id", "promotion-record.json")
            != record_str(&queue, "/queue_entry_id", &queue_name)
            || record_str(promotion, "/integration_record_id", "promotion-record.json")
                != record_str(&integration, "/integration_id", &integration_name)
            || record_str(
                promotion,
                "/mainline_receipt_record_id",
                "promotion-record.json",
            ) != record_str(&receipt, "/receipt_id", &receipt_name)
        {
            fail("PROMOTION_PARALLEL_MERGE_REFERENCE_MISMATCH");
        }
    }
    if !(record_time(&worktree, &worktree_name) <= record_time(&reproduction, &reproduction_name)
        && record_time(&reproduction, &reproduction_name)
            <= record_time(&candidate, &candidate_name)
        && record_time(&candidate, &candidate_name) <= record_time(review, "review-record.json")
        && record_time(review, "review-record.json")
            <= record_time(&effectiveness, &effectiveness_name)
        && record_time(&effectiveness, &effectiveness_name) <= record_time(&merge, &merge_name)
        && record_time(&merge, &merge_name) <= record_time(promotion, "promotion-record.json"))
    {
        fail("FIX_LIFECYCLE_ORDER_INVALID");
    }
}

fn assert_record_graph(
    root: &Path,
    module_id: Option<&str>,
    artifact: &Value,
    require_freeze: bool,
) {
    if let Some(module_id) = module_id {
        let _ = read_record(root, &module_record_name("worktree-record", module_id));
    }
    let evidence_name = module_id
        .map(|id| module_record_name("evidence-record", id))
        .unwrap_or_else(|| "evidence-record.json".into());
    let review_name = module_id
        .map(|id| module_record_name("review-record", id))
        .unwrap_or_else(|| "review-record.json".into());
    let promotion_name = module_id
        .map(|id| module_record_name("promotion-record", id))
        .unwrap_or_else(|| "promotion-record.json".into());
    let evidence = read_record(root, &evidence_name);
    let review = read_record(root, &review_name);
    let promotion = read_record(root, &promotion_name);
    assert_record_schema(&evidence, &review, &promotion);
    if let Some(module_id) = module_id {
        assert_fix_lifecycle_graph(root, module_id, &review, &promotion, artifact);
    }
    let cleanup_id = record_str(
        &promotion,
        "/playground_cleanup_record_id",
        "promotion-record.json",
    );
    let cleanup = read_record(root, &format!("playground-cleanup-{}.json", cleanup_id));
    if record_str(&cleanup, "/cleanup_id", "playground-cleanup-record") != cleanup_id
        || !matches!(
            cleanup.get("disposition").and_then(Value::as_str),
            Some("archive_then_remove" | "remove" | "retain_open")
        )
    {
        fail("INVALID_PLAYGROUND_CLEANUP_RECORD");
    }
    let evidence_id = record_str(&evidence, "/evidence_id", "evidence-record.json");
    let issue_id = record_str(&evidence, "/issue_id", "evidence-record.json");
    let experiment_id = record_str(&evidence, "/experiment_id", "evidence-record.json");
    if record_str(&evidence, "/result", "evidence-record.json") != "pass"
        || record_str(&review, "/verdict", "review-record.json") != "pass"
    {
        fail("PROMOTION_EVIDENCE_NOT_PASSED");
    }
    if record_str(&review, "/issue_id", "review-record.json") != issue_id
        || record_str(&promotion, "/issue_id", "promotion-record.json") != issue_id
        || record_str(&promotion, "/experiment_id", "promotion-record.json") != experiment_id
    {
        fail("RECORD_GRAPH_SCOPE_MISMATCH");
    }
    if record_str(&review, "/promotion_id", "review-record.json")
        != record_str(&promotion, "/promotion_id", "promotion-record.json")
        || record_str(&review, "/review_id", "review-record.json")
            != record_str(&promotion, "/review_id", "promotion-record.json")
    {
        fail("RECORD_GRAPH_REFERENCE_MISMATCH");
    }
    let review_evidence_ids = record_array(&review, "/evidence_ids", "review-record.json");
    let promotion_evidence_ids = record_array(&promotion, "/evidence_ids", "promotion-record.json");
    if !review_evidence_ids
        .iter()
        .any(|id| id.as_str() == Some(evidence_id))
        || !promotion_evidence_ids
            .iter()
            .any(|id| id.as_str() == Some(evidence_id))
    {
        fail("RECORD_GRAPH_EVIDENCE_REFERENCE_MISMATCH");
    }
    if let Some(module_id) = module_id {
        if record_str(&promotion, "/module_id", "promotion-record.json") != module_id
            || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
        {
            fail("RECORD_GRAPH_MODULE_MISMATCH");
        }
    }
    let artifact_hash = record_str(artifact, "/artifact_hash", "artifact");
    if record_str(&review, "/reviewed_artifact_hash", "review-record.json") != artifact_hash
        || record_str(&promotion, "/artifact_hash", "promotion-record.json") != artifact_hash
    {
        fail("RECORD_GRAPH_ARTIFACT_MISMATCH");
    }
    if record_str(&review, "/reviewed_commit", "review-record.json")
        != record_str(&promotion, "/source_commit", "promotion-record.json")
        || record_str(&evidence, "/source_commit", "evidence-record.json")
            != record_str(&promotion, "/source_commit", "promotion-record.json")
        || record_str(&review, "/reviewed_scope_hash", "review-record.json")
            != record_str(&promotion, "/scope_hash", "promotion-record.json")
        || record_str(&evidence, "/scope_hash", "evidence-record.json")
            != record_str(&promotion, "/scope_hash", "promotion-record.json")
    {
        fail("RECORD_GRAPH_INPUT_MISMATCH");
    }
    let confidence = review
        .get("ai_confidence")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| fail("INVALID_REVIEW_CONFIDENCE"));
    if !(0.0..=1.0).contains(&confidence)
        || record_str(&review, "/confidence_rationale", "review-record.json").is_empty()
    {
        fail("INVALID_REVIEW_CONFIDENCE");
    }
    let gates = record_array(
        &promotion,
        "/required_gate_results",
        "promotion-record.json",
    );
    if gates
        .iter()
        .any(|gate| gate.get("result").and_then(Value::as_str) != Some("pass"))
    {
        fail("PROMOTION_GATE_NOT_PASSED");
    }
    let expires_at = record_str(&evidence, "/expires_at", "evidence-record.json");
    let expires_at = DateTime::parse_from_rfc3339(expires_at)
        .unwrap_or_else(|_| fail("INVALID_EVIDENCE_EXPIRY"))
        .with_timezone(&Utc);
    if expires_at <= Utc::now() {
        fail("EVIDENCE_EXPIRED");
    }
    if require_freeze {
        let module_id = module_id.unwrap_or_else(|| fail("FREEZE_RECORD_MODULE_REQUIRED"));
        let project = read_project(root);
        let module = project
            .get("modules")
            .and_then(Value::as_array)
            .and_then(|modules| {
                modules.iter().find(|module| {
                    module.get("module_id").and_then(Value::as_str) == Some(module_id)
                })
            })
            .unwrap_or_else(|| fail("MODULE_NOT_FOUND"));
        let (regression, regression_hash) =
            assert_regression_report(root, module_id, module, &promotion, artifact);
        let freeze_name = freeze_record_name(module_id);
        let freeze = read_record(root, &freeze_name);
        let active_root = contract_root(root, &project, "/governance/active_root");
        let active_version = record_str(&freeze, "/active_version", &freeze_name);
        let active_artifact = active_root
            .join(module_id)
            .join(active_version)
            .join("artifact.json");
        if active_artifact.is_file()
            && !fs::symlink_metadata(&active_artifact)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
        {
            let active_value: Value = serde_json::from_str(
                &fs::read_to_string(&active_artifact)
                    .unwrap_or_else(|_| fail("ACTIVE_ARTIFACT_MISSING")),
            )
            .unwrap_or_else(|_| fail("INVALID_ACTIVE_ARTIFACT"));
            if record_str(&active_value, "/artifact_hash", "active_artifact")
                != record_str(&freeze, "/library_hash", &freeze_name)
            {
                fail("FREEZE_ACTIVE_HASH_MISMATCH");
            }
        }
        for path in [
            "/freeze_id",
            "/issue_id",
            "/module_id",
            "/promotion_id",
            "/promotion_record_hash",
            "/artifact_record_id",
            "/regression_report_id",
            "/regression_report_hash",
            "/source_commit_or_tag",
            "/active_version",
            "/library_hash",
            "/public_api_hash",
            "/review_id",
            "/created_at",
            "/clean_scope/base_commit",
            "/clean_scope/generated_policy",
        ] {
            record_str(&freeze, path, &freeze_name);
        }
        for path in ["/previous_active_immutable", "/git_clean"] {
            if freeze
                .get(path.trim_start_matches('/'))
                .and_then(Value::as_bool)
                .is_none()
            {
                fail(format!("INVALID_RECORD:{}:{}", freeze_name, path));
            }
        }
        let clean_scope = freeze
            .get("clean_scope")
            .and_then(Value::as_object)
            .unwrap_or_else(|| fail(format!("INVALID_RECORD:{}:/clean_scope", freeze_name)));
        for key in ["changed_paths", "ignored_paths"] {
            if clean_scope.get(key).and_then(Value::as_array).is_none() {
                fail(format!(
                    "INVALID_RECORD:{}/clean_scope/{}",
                    freeze_name, key
                ));
            }
        }
        let owners = freeze
            .get("owners")
            .and_then(Value::as_object)
            .unwrap_or_else(|| fail(format!("INVALID_RECORD:{}:/owners", freeze_name)));
        for key in [
            "vcs",
            "compiler",
            "api_extractor",
            "review",
            "artifact_registry",
        ] {
            if owners
                .get(key)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .is_none()
            {
                fail(format!("INVALID_RECORD:{}:/owners/{}", freeze_name, key));
            }
        }
        if record_str(&freeze, "/module_id", &freeze_name) != module_id {
            fail("FREEZE_RECORD_MODULE_MISMATCH");
        }
        if record_str(&freeze, "/promotion_id", &freeze_name)
            != record_str(&promotion, "/promotion_id", "promotion-record.json")
        {
            fail("FREEZE_RECORD_PROMOTION_MISMATCH");
        }
        if record_str(&freeze, "/review_id", &freeze_name)
            != record_str(&review, "/review_id", "review-record.json")
        {
            fail("FREEZE_RECORD_REVIEW_MISMATCH");
        }
        if record_str(&freeze, "/library_hash", &freeze_name) != artifact_hash {
            fail("FREEZE_RECORD_LIBRARY_HASH_MISMATCH");
        }
        if record_str(&freeze, "/active_version", &freeze_name)
            != record_str(&promotion, "/new_active_version", "promotion-record.json")
        {
            fail("FREEZE_RECORD_VERSION_MISMATCH");
        }
        if record_str(&freeze, "/artifact_record_id", &freeze_name) != evidence_id {
            fail("FREEZE_RECORD_ARTIFACT_RECORD_MISMATCH");
        }
        if record_str(&freeze, "/regression_report_id", &freeze_name)
            != record_str(
                &regression,
                "/regression_report_id",
                "regression-report.json",
            )
            || record_str(&freeze, "/regression_report_hash", &freeze_name) != regression_hash
        {
            fail("FREEZE_RECORD_REGRESSION_REPORT_MISMATCH");
        }
        if record_str(&freeze, "/promotion_record_hash", &freeze_name)
            != sha256(&canonical(&promotion))
        {
            fail("FREEZE_RECORD_PROMOTION_HASH_MISMATCH");
        }
        if record_str(&freeze, "/public_api_hash", &freeze_name)
            != record_str(&promotion, "/public_api_hash", "promotion-record.json")
        {
            fail("FREEZE_RECORD_PUBLIC_API_HASH_MISMATCH");
        }
        if record_str(&freeze, "/source_commit_or_tag", &freeze_name).is_empty()
            || record_str(&freeze, "/public_api_hash", &freeze_name).is_empty()
        {
            fail("FREEZE_RECORD_REQUIRED_FIELD_MISMATCH");
        }
        if record_str(&freeze, "/source_commit_or_tag", &freeze_name)
            != record_str(&promotion, "/source_commit", "promotion-record.json")
        {
            fail("FREEZE_RECORD_SOURCE_COMMIT_MISMATCH");
        }
        if freeze.get("git_clean") != Some(&Value::Bool(true)) {
            fail("FREEZE_REQUIREMENTS_NOT_MET");
        }
        let previous_version = freeze
            .get("previous_active_version")
            .and_then(Value::as_str);
        let previous_immutable = freeze
            .get("previous_active_immutable")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| fail("FREEZE_REQUIREMENTS_NOT_MET"));
        if previous_immutable != previous_version.is_some() {
            fail("FREEZE_PREVIOUS_ACTIVE_CLAIM_MISMATCH");
        }
        if let Some(version_base) = module.get("version_base") {
            if freeze
                .get("previous_active_version")
                .and_then(Value::as_str)
                != version_base
                    .get("previous_active_version")
                    .and_then(Value::as_str)
                || promotion
                    .get("previous_active_version")
                    .and_then(Value::as_str)
                    != version_base
                        .get("previous_active_version")
                        .and_then(Value::as_str)
                || promotion.get("new_active_version").and_then(Value::as_str)
                    != version_base
                        .get("new_active_version")
                        .and_then(Value::as_str)
                || promotion.get("base_artifact_hash").and_then(Value::as_str)
                    != version_base
                        .get("base_artifact_hash")
                        .and_then(Value::as_str)
                || promotion.get("base_commit").and_then(Value::as_str)
                    != version_base
                        .get("base_source_commit")
                        .and_then(Value::as_str)
            {
                fail("MODULE_VERSION_RECORD_MISMATCH");
            }
        }
        if let Some(previous) = freeze
            .get("previous_active_version")
            .and_then(Value::as_str)
        {
            let project = read_project(root);
            let active_root = contract_root(root, &project, "/governance/active_root");
            let previous_path = active_root.join(module_id).join(previous);
            assert_no_symlink_components(root, &previous_path, "previous_active");
            if !previous_path.is_dir() {
                fail("PREVIOUS_ACTIVE_MISSING");
            }
            let artifact = previous_path.join("artifact.json");
            if fs::symlink_metadata(&artifact)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                fail("PREVIOUS_ACTIVE_SYMLINK");
            }
            if !artifact.is_file() {
                fail("PREVIOUS_ACTIVE_ARTIFACT_MISSING");
            }
            let previous_value: Value = serde_json::from_str(
                &fs::read_to_string(&artifact)
                    .unwrap_or_else(|_| fail("PREVIOUS_ACTIVE_ARTIFACT_MISSING")),
            )
            .unwrap_or_else(|_| fail("INVALID_PREVIOUS_ACTIVE_ARTIFACT"));
            if previous_value
                .get("module_id")
                .and_then(Value::as_str)
                .is_some()
            {
                let module = project
                    .get("modules")
                    .and_then(Value::as_array)
                    .and_then(|modules| {
                        modules.iter().find(|module| {
                            module.get("module_id").and_then(Value::as_str) == Some(module_id)
                        })
                    })
                    .unwrap_or_else(|| fail("MODULE_NOT_FOUND"));
                previous_active_matches_module(module, &previous_value);
            } else {
                assert_artifact_matches(&project, &previous_value);
            }
            let previous_hash = record_str(
                &previous_value,
                "/artifact_hash",
                "previous_active_artifact",
            );
            let promotion = read_record(root, &module_record_name("promotion-record", module_id));
            if promotion
                .pointer("/base_artifact_hash")
                .and_then(Value::as_str)
                != Some(previous_hash)
            {
                fail("PREVIOUS_ACTIVE_HASH_MISMATCH");
            }
            if record_str(&previous_value, "/module_id", "previous_active_artifact") != module_id {
                fail("PREVIOUS_ACTIVE_MODULE_MISSING");
            }
        }
    }
}

fn promote(root: &Path, target: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    let project = read_project(root);
    assert_project_contract(root, &project);
    assert_goal_confirmed(root);
    assert_declared_contracts(root, &project, true);
    let from = required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT");
    let valid = matches!(
        (from, target),
        ("draft", "source_implemented")
            | ("source_implemented", "contract_bound")
            | ("contract_bound", "compiled")
            | ("compiled", "controlled_verified")
            | ("controlled_verified", "architecture_stable")
    );
    if !valid {
        if target == "frozen" {
            fail("PROJECT_FREEZE_REQUIRES_MODULE_FREEZE");
        }
        fail(format!("INVALID_LIFECYCLE_TRANSITION:{}->{}", from, target));
    }
    let mut candidate = project.clone();
    candidate["lifecycle"]["stage"] = Value::String(target.into());
    if matches!(
        target,
        "compiled" | "controlled_verified" | "architecture_stable"
    ) {
        assert_compile_preconditions(root, &candidate, None);
        let artifact = build_artifact(&candidate);
        if target == "architecture_stable" {
            assert_record_graph(root, None, &artifact, false);
        }
        write_artifact_value(root, &candidate, &artifact);
    }
    write_project(root, &candidate);
    println!("{}", serde_json::to_string_pretty(&candidate).unwrap());
}

fn promote_module(root: &Path, module_id: &str, target: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    if target == "frozen" && recover_freeze_transaction(root, &project, module_id) {
        println!("{}", serde_json::to_string_pretty(&project).unwrap());
        return;
    }
    assert_declared_contracts(root, &project, true);
    if target == "frozen" {
        freeze_module(root, module_id);
        return;
    }
    if target == "retired" {
        fail(format!(
            "MODULE_RETIRE_REQUIRES_VERSIONED_ARTIFACT:{}",
            module_id
        ));
    }
    assert_goal_confirmed(root);
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let index = modules
        .iter()
        .position(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    let from = modules[index]
        .get("stage")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail(format!("INVALID_MODULE_CONTRACT:{}", module_id)));
    let valid = matches!(
        (from, target),
        ("draft", "source_implemented")
            | ("source_implemented", "contract_bound")
            | ("contract_bound", "compiled")
            | ("compiled", "controlled_verified")
            | ("controlled_verified", "architecture_stable")
    );
    if !valid {
        fail(format!("INVALID_LIFECYCLE_TRANSITION:{}->{}", from, target));
    }
    let project_stage = required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT");
    if matches!(
        target,
        "compiled" | "controlled_verified" | "architecture_stable"
    ) && !matches!(
        project_stage,
        "contract_bound" | "compiled" | "controlled_verified" | "architecture_stable"
    ) {
        fail(format!(
            "MODULE_COMPILE_REQUIRES_PROJECT_CONTRACT:{}:{}",
            module_id, project_stage
        ));
    }
    let mut candidate = project.clone();
    candidate["modules"][index]["stage"] = Value::String(target.into());
    assert_compile_preconditions(root, &candidate, Some(module_id));
    let artifact = build_artifact(&candidate);
    let module_artifact = if matches!(
        target,
        "contract_bound" | "compiled" | "controlled_verified" | "architecture_stable"
    ) {
        let module_artifact = read_module_artifact(root, &project, module_id);
        let mut staged = module_artifact_matches_project(&modules[index], &module_artifact);
        staged["stage"] = Value::String(target.into());
        write_module_artifact_value(root, &project, module_id, &staged);
        Some(staged)
    } else {
        None
    };
    if target == "architecture_stable" {
        assert_fix_architecture_gate(
            root,
            module_id,
            module_artifact.as_ref().unwrap_or(&artifact),
        );
    }
    write_artifact_value(root, &project, &artifact);
    write_project(root, &candidate);
    println!("{}", serde_json::to_string_pretty(&candidate).unwrap());
}

fn freeze_module(root: &Path, module_id: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    if recover_freeze_transaction(root, &project, module_id) {
        println!("{}", serde_json::to_string_pretty(&project).unwrap());
        return;
    }
    assert_declared_contracts(root, &project, true);
    assert_goal_confirmed(root);
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let index = modules
        .iter()
        .position(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    let stage = modules[index]
        .get("stage")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail(format!("INVALID_MODULE_CONTRACT:{}", module_id)));
    if stage != "architecture_stable" {
        fail(format!(
            "MODULE_NOT_READY_TO_FREEZE:{}:{}",
            module_id, stage
        ));
    }
    assert_vcs_clean(root, &project, module_id);
    if matches!(
        required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT"),
        "frozen" | "retired"
    ) {
        fail("PROJECT_ALREADY_FROZEN");
    }
    let mut candidate = project.clone();
    candidate["modules"][index]["stage"] = Value::String("frozen".into());
    assert_compile_preconditions(root, &project, Some(module_id));
    let promoted_artifact = read_compiled_artifact(root, &project);
    assert_artifact_matches(&project, &promoted_artifact);
    let module_artifact = module_artifact_matches_project(
        &project["modules"][index],
        &read_module_artifact(root, &project, module_id),
    );
    if module_artifact.get("stage").and_then(Value::as_str) != Some("architecture_stable") {
        fail(format!(
            "MODULE_ARTIFACT_NOT_ARCHITECTURE_STABLE:{}",
            module_id
        ));
    }
    let mut staged_module_artifact = module_artifact.clone();
    staged_module_artifact["stage"] = Value::String("frozen".into());
    let module_artifact = staged_module_artifact.clone();
    let archive = contract_root(root, &project, "/governance/protected_root")
        .join("history")
        .join(module_id);
    let staging_archive = contract_root(root, &project, "/governance/protected_root")
        .join("history")
        .join(format!(".{}.staging.{}", module_id, std::process::id()));
    assert_no_symlink_components(root, &archive, "protected_archive");
    assert_no_symlink_components(root, &staging_archive, "protected_archive_staging");
    if archive.exists() {
        fail(format!("PROTECTED_HISTORY_IMMUTABLE:{}", module_id));
    }
    if staging_archive.exists() {
        fail(format!("PROTECTED_ARCHIVE_STAGING_EXISTS:{}", module_id));
    }
    let review_name = module_record_name("review-record", module_id);
    let promotion_name = module_record_name("promotion-record", module_id);
    let review = read_record(root, &review_name);
    let promotion = read_record(root, &promotion_name);
    let (regression, regression_hash) = assert_regression_report(
        root,
        module_id,
        &candidate["modules"][index],
        &promotion,
        &module_artifact,
    );
    let freeze_name = freeze_record_name(module_id);
    let mut freeze = read_record(root, &freeze_name);
    let artifact = module_artifact.clone();
    let reviewed_hash = record_str(&artifact, "/artifact_hash", "module-artifact");
    if record_str(&review, "/reviewed_artifact_hash", "review-record.json") != reviewed_hash
        || record_str(&promotion, "/artifact_hash", "promotion-record.json") != reviewed_hash
    {
        fail("RECORD_GRAPH_ARTIFACT_MISMATCH");
    }
    freeze["library_hash"] = Value::String(reviewed_hash.into());
    if record_str(&freeze, "/public_api_hash", &freeze_name)
        != record_str(&promotion, "/public_api_hash", "promotion-record.json")
    {
        fail("FREEZE_RECORD_PUBLIC_API_HASH_MISMATCH");
    }
    freeze["promotion_record_hash"] = Value::String(sha256(&canonical(&promotion)));
    freeze["regression_report_id"] = Value::String(
        record_str(
            &regression,
            "/regression_report_id",
            "regression-report.json",
        )
        .into(),
    );
    freeze["regression_report_hash"] = Value::String(regression_hash);
    assert_protected_not_ignored(root, &archive);
    stage_protected_archive(
        root,
        &project,
        &candidate["modules"][index],
        module_id,
        &artifact,
        &freeze,
        &staging_archive,
    );
    let transaction = freeze_transaction_dir(root, module_id);
    let backup = transaction.join("backup");
    fs::create_dir_all(&backup).unwrap_or_else(|_| fail("FREEZE_TRANSACTION_FAILED"));
    for (name, source) in [
        ("project.json", root.join(".appsdk/project.json")),
        (
            "project.compiled.json",
            generated_root(root, &project).join("project.compiled.json"),
        ),
        (
            "module.compiled.json",
            module_artifact_file(root, &project, module_id),
        ),
        (
            "review-record.json",
            root.join(".appsdk/records")
                .join(module_record_name("review-record", module_id)),
        ),
        (
            "promotion-record.json",
            root.join(".appsdk/records")
                .join(module_record_name("promotion-record", module_id)),
        ),
        (
            "regression-report.json",
            root.join(".appsdk/records")
                .join(module_record_name("regression-report", module_id)),
        ),
        (
            "freeze-record.json",
            root.join(".appsdk/records").join(&freeze_name),
        ),
    ] {
        fs::copy(source, backup.join(name)).unwrap_or_else(|_| fail("FREEZE_TRANSACTION_FAILED"));
    }
    atomic_write_json(
        &transaction.join("marker.json"),
        &serde_json::json!({"phase":"prepared","pid":std::process::id()}),
        "FREEZE_TRANSACTION_FAILED",
    );
    write_module_artifact_value(root, &project, module_id, &module_artifact);
    write_artifact_value(root, &candidate, &build_artifact(&candidate));
    write_record(root, &review_name, &review);
    write_record(root, &promotion_name, &promotion);
    write_record(
        root,
        &module_record_name("regression-report", module_id),
        &regression,
    );
    write_record(root, &freeze_record_name(module_id), &freeze);
    assert_record_graph(root, Some(module_id), &artifact, true);
    write_project(root, &candidate);
    atomic_write_json(
        &transaction.join("marker.json"),
        &serde_json::json!({"phase":"commit_ready","pid":std::process::id()}),
        "FREEZE_TRANSACTION_FAILED",
    );
    fs::rename(&staging_archive, &archive).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
    fs::remove_dir_all(&transaction).unwrap_or_else(|_| fail("FREEZE_TRANSACTION_CLEANUP_FAILED"));
    println!("{}", serde_json::to_string_pretty(&candidate).unwrap());
}

fn publish_active(root: &Path, module_id: &str, version: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    assert_version(version, "INVALID_ACTIVE_VERSION");
    let project = read_project(root);
    assert_project_contract(root, &project);
    assert_declared_contracts(root, &project, true);
    let stage = required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT");
    assert_sdk_lock(
        root,
        &project,
        matches!(
            stage,
            "compiled" | "controlled_verified" | "architecture_stable" | "frozen" | "retired"
        ),
    );
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let module = modules
        .iter()
        .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    if module.get("stage").and_then(Value::as_str) != Some("frozen") {
        fail(format!(
            "ACTIVE_PUBLISH_REQUIRES_FROZEN_MODULE:{}",
            module_id
        ));
    }
    if let Some(version_base) = module.get("version_base") {
        if version_base
            .get("new_active_version")
            .and_then(Value::as_str)
            != Some(version)
        {
            fail("ACTIVE_VERSION_BASE_MISMATCH");
        }
        let previous = record_str(
            version_base,
            "/previous_active_version",
            "module-version-base",
        );
        let previous_artifact = contract_root(root, &project, "/governance/active_root")
            .join(module_id)
            .join(previous)
            .join("artifact.json");
        let previous_value: Value = serde_json::from_str(
            &fs::read_to_string(previous_artifact)
                .unwrap_or_else(|_| fail("PREVIOUS_ACTIVE_ARTIFACT_MISSING")),
        )
        .unwrap_or_else(|_| fail("INVALID_PREVIOUS_ACTIVE_ARTIFACT"));
        if record_str(
            &previous_value,
            "/artifact_hash",
            "previous_active_artifact",
        ) != record_str(version_base, "/base_artifact_hash", "module-version-base")
        {
            fail("PREVIOUS_ACTIVE_HASH_MISMATCH");
        }
    }
    let artifact = read_module_artifact(root, &project, module_id);
    module_artifact_matches_project(module, &artifact);
    if artifact.get("stage").and_then(Value::as_str) != Some("frozen") {
        fail(format!(
            "ACTIVE_PUBLISH_REQUIRES_FROZEN_MODULE_ARTIFACT:{}",
            module_id
        ));
    }
    assert_record_graph(root, Some(module_id), &artifact, true);
    let artifact_hash = record_str(&artifact, "/artifact_hash", "module-artifact");
    if record_str(
        &read_record(root, &freeze_record_name(module_id)),
        "/active_version",
        &freeze_record_name(module_id),
    ) != version
        || record_str(
            &read_record(root, &module_record_name("promotion-record", module_id)),
            "/new_active_version",
            "promotion-record.json",
        ) != version
    {
        fail("ACTIVE_VERSION_RECORD_MISMATCH");
    }
    let active_base = contract_root(&root, &project, "/governance/active_root");
    let active = active_base.join(module_id).join(version);
    let index = active_base.join(module_id).join("current.json");
    assert_no_symlink_components(&root, &active_base.join(module_id), "active_module");
    assert_no_symlink_components(&root, &active, "active_version");
    assert_no_symlink_components(&root, &index, "active_index");
    fs::create_dir_all(index.parent().unwrap()).unwrap_or_else(|_| fail("ACTIVE_PUBLISH_FAILED"));
    let lock = index.with_extension("publish.lock");
    let lock_exists = lock.exists();
    if lock_exists {
        let stale = fs::metadata(&lock)
            .and_then(|metadata| metadata.modified())
            .and_then(|modified| modified.elapsed().map_err(std::io::Error::other))
            .map(|age| age > std::time::Duration::from_secs(300))
            .unwrap_or(false);
        if stale {
            fs::remove_file(&lock)
                .unwrap_or_else(|_| fail(format!("ACTIVE_PUBLISH_BUSY:{}", module_id)));
        }
    }
    let mut lock_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
        .unwrap_or_else(|_| fail(format!("ACTIVE_PUBLISH_BUSY:{}", module_id)));
    if let Err(error) = lock_file.write_all(artifact_hash.as_bytes()) {
        let _ = fs::remove_file(&lock);
        fail(format!("ACTIVE_PUBLISH_FAILED:{}", error));
    }
    let mut active_created = false;
    let staging = staging_path(&root, &project, module_id);
    let publish_result: Result<(), String> = (|| {
        assert_no_symlink_components(
            &root,
            &active_base.join(module_id),
            "active_module_before_write",
        );
        assert_no_symlink_components(
            &root,
            &staging_path(&root, &project, module_id),
            "active_staging_before_write",
        );
        if index.exists() {
            let current: Value = serde_json::from_str(
                &fs::read_to_string(&index).map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?,
            )
            .map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?;
            if current.get("version").and_then(Value::as_str) == Some(version) {
                return Err(format!("ACTIVE_VERSION_EXISTS:{}", version));
            }
        }
        if active.exists() {
            return Err(format!("ACTIVE_VERSION_EXISTS:{}", version));
        }
        assert_no_symlink_components(&root, &staging, "active_staging");
        if staging.exists() {
            return Err("ACTIVE_PUBLISH_STAGING_EXISTS".into());
        }
        fs::create_dir_all(&staging).map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?;
        fs::write(
            staging.join("artifact.json"),
            serde_json::to_string_pretty(&artifact).unwrap() + "\n",
        )
        .map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?;
        let artifacts = artifact
            .get("artifacts")
            .and_then(Value::as_array)
            .ok_or("ACTIVE_PUBLISH_FAILED".to_string())?;
        fs::create_dir_all(staging.join("lib")).map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?;
        for entry in artifacts {
            let relative = entry
                .get("path")
                .and_then(Value::as_str)
                .ok_or("ACTIVE_PUBLISH_FAILED".to_string())?;
            let source = safe_module_artifact_path(&root, &project, module_id, relative);
            let target = staging.join("lib").join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?;
            }
            fs::copy(&source, &target).map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?;
        }
        fs::write(
            staging.join("current.json"),
            format!(
                "{{\"module_id\":\"{}\",\"version\":\"{}\",\"artifact_hash\":\"{}\"}}\n",
                module_id, version, artifact_hash
            ),
        )
        .map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?;
        fs::create_dir_all(active.parent().unwrap())
            .map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?;
        fs::rename(&staging, &active).map_err(|_| "ACTIVE_VERSION_EXISTS".to_string())?;
        active_created = true;
        assert_no_symlink_components(&root, &active, "active_version_after_rename");
        assert_no_symlink_components(&root, &index, "active_index_before_write");
        let index_contents = fs::read_to_string(active.join("current.json"))
            .map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?;
        let current_tmp = active.with_extension("current.json.tmp");
        fs::write(&current_tmp, index_contents).map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?;
        fs::rename(&current_tmp, &index).map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?;
        fs::remove_file(active.join("current.json"))
            .map_err(|_| "ACTIVE_PUBLISH_FAILED".to_string())?;
        Ok(())
    })();
    if let Err(error) = publish_result {
        let _ = fs::remove_file(&lock);
        let _ = fs::remove_dir_all(&staging);
        if active_created {
            let _ = fs::remove_file(&index);
            let _ = fs::remove_dir_all(&active);
        }
        fail(error);
    }
    fs::remove_file(lock).unwrap_or_else(|_| fail("ACTIVE_PUBLISH_FAILED"));
    if module.get("version_base").is_some() {
        let mut candidate = project.clone();
        candidate["modules"][modules
            .iter()
            .position(|entry| entry.get("module_id").and_then(Value::as_str) == Some(module_id))
            .unwrap_or_else(|| fail("MODULE_NOT_FOUND"))]
        .as_object_mut()
        .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT"))
        .remove("version_base");
        write_project(root, &candidate);
    }
    println!("active {} {}", module_id, version);
}

fn assert_sdk_resources(root: &Path, required: bool) {
    let path = root.join(".appsdk/sdk-resources.json");
    if !path.exists() {
        if required {
            fail("MISSING_SDK_RESOURCES");
        }
        return;
    }
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_resources");
    }
    let record: Value = serde_json::from_str(
        &fs::read_to_string(&path).unwrap_or_else(|_| fail("INVALID_SDK_RESOURCES")),
    )
    .unwrap_or_else(|_| fail("INVALID_SDK_RESOURCES"));
    assert_bundle_manifest();
    if record.get("schema_version").and_then(Value::as_u64) != Some(1)
        || record.get("sdk").and_then(Value::as_str) != Some("appsdk")
        || record.get("version").and_then(Value::as_str) != Some("0.1.6")
        || record.get("bundle_digest").and_then(Value::as_str) != Some(&sdk_bundle_digest())
        || record.get("manifest_digest").and_then(Value::as_str)
            != Some(&digest_bytes(SDK_BUNDLE_MANIFEST.as_bytes()))
    {
        fail("SDK_RESOURCES_BUNDLE_MISMATCH");
    }
    let entries = record
        .get("resources")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_SDK_RESOURCES"));
    let bundle_entries = sdk_bundle_resource_entries();
    if entries.len() != bundle_entries.len() {
        fail("SDK_RESOURCE_SET_MISMATCH");
    }
    for (source, class, content) in bundle_entries {
        let entry = entries
            .iter()
            .find(|entry| {
                entry.get("source").and_then(Value::as_str) == Some(source.as_str())
                    && entry.get("class").and_then(Value::as_str) == Some(class.as_str())
            })
            .unwrap_or_else(|| fail(format!("SDK_RESOURCE_MISSING:{}", source)));
        let relative = entry
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("INVALID_SDK_RESOURCES"));
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            || (relative != ".appsdk" && !relative.starts_with(".appsdk/"))
            || relative != sdk_resource_install_relative(&source, &class)
        {
            fail(format!("SDK_RESOURCE_PATH_ESCAPE:{}", relative));
        }
        let expected = digest_bytes(content.as_bytes());
        if entry.get("digest").and_then(Value::as_str) != Some(expected.as_str()) {
            fail(format!("SDK_RESOURCE_DIGEST_RECORD_MISMATCH:{}", source));
        }
        let target = root.join(relative);
        assert_no_symlink_components(root, &target, "sdk_resource_record");
        if !target.is_file() || file_sha256(&target, "sdk_resource") != expected {
            fail(format!("SDK_RESOURCE_MISMATCH:{}", relative));
        }
    }
}

fn verify_internal(root: &Path, admission: bool, emit_result: bool) {
    assert_project_root_safe(root);
    let project = read_project(root);
    assert_governance_maps(root);
    let _ = assert_sdk_migration_record(root);
    assert_declared_contracts(root, &project, true);
    assert_project_contract(root, &project);
    if project.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail("UNSUPPORTED_PROJECT_SCHEMA");
    }
    if required_str(&project, "/sdk/name", "INVALID_SDK_CONTRACT") != "appsdk"
        || project
            .pointer("/sdk/bundle_manifest")
            .and_then(Value::as_str)
            != Some(".appsdk/contracts/sdk-bundle.manifest.json")
        || project
            .pointer("/sdk/resource_record")
            .and_then(Value::as_str)
            != Some(".appsdk/sdk-resources.json")
    {
        fail("INVALID_SDK_CONTRACT");
    }
    if project
        .pointer("/access/protected_paths")
        .and_then(Value::as_array)
        .is_none()
        || project
            .pointer("/governance/playground_root")
            .and_then(Value::as_str)
            .is_none()
        || project
            .pointer("/governance/active_root")
            .and_then(Value::as_str)
            .is_none()
        || project
            .pointer("/governance/protected_root")
            .and_then(Value::as_str)
            .is_none()
        || project
            .pointer("/governance/generated_root")
            .and_then(Value::as_str)
            .is_none()
        || project
            .pointer("/governance/active_kind")
            .and_then(Value::as_str)
            != Some("immutable_consumable_library")
        || project
            .pointer("/lifecycles/issue")
            .and_then(Value::as_str)
            .is_none()
        || project
            .pointer("/lifecycles/library")
            .and_then(Value::as_str)
            .is_none()
        || project
            .pointer("/lifecycles/source_snapshot")
            .and_then(Value::as_str)
            .is_none()
        || project
            .pointer("/lifecycles/artifact")
            .and_then(Value::as_str)
            .is_none()
    {
        fail("INVALID_GOVERNANCE_CONTRACT");
    }
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let mut module_ids = std::collections::HashSet::new();
    for module in modules {
        let module_id = module
            .get("module_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        assert_identifier(module_id, &format!("INVALID_MODULE_CONTRACT:{}", module_id));
        if !module_ids.insert(module_id) {
            fail(format!("DUPLICATE_MODULE:{}", module_id));
        }
        for key in ["module_id", "stage", "source_owner", "active_artifact"] {
            if module.get(key).and_then(Value::as_str).is_none() {
                fail(format!("INVALID_MODULE_CONTRACT:{}", key));
            }
        }
        let stage = module.get("stage").and_then(Value::as_str).unwrap_or("");
        if !matches!(
            stage,
            "draft"
                | "source_implemented"
                | "contract_bound"
                | "compiled"
                | "controlled_verified"
                | "architecture_stable"
                | "frozen"
                | "retired"
        ) {
            fail(format!("INVALID_MODULE_CONTRACT:{}", module_id));
        }
        if module.get("source_owner").and_then(Value::as_str) != Some(module_id)
            || module
                .get("owned_paths")
                .and_then(Value::as_array)
                .map(|values| {
                    values.is_empty()
                        || values.iter().any(|value| {
                            value.as_str().map(|entry| entry.is_empty()).unwrap_or(true)
                        })
                })
                .unwrap_or(true)
            || module
                .get("generated_outputs")
                .and_then(Value::as_array)
                .map(|values| {
                    values.is_empty()
                        || values.iter().any(|value| {
                            value.as_str().map(|entry| entry.is_empty()).unwrap_or(true)
                        })
                })
                .unwrap_or(true)
            || module
                .get("active_artifact")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
            || module
                .get("contract_paths")
                .and_then(Value::as_array)
                .map(|values| {
                    values.is_empty()
                        || values.iter().any(|value| {
                            value.as_str().map(|entry| entry.is_empty()).unwrap_or(true)
                        })
                })
                .unwrap_or(true)
            || module.get("build").and_then(Value::as_object).is_none()
            || module
                .get("artifact_paths")
                .and_then(Value::as_array)
                .map(|values| {
                    values.is_empty()
                        || values.iter().any(|value| {
                            value.as_str().map(|entry| entry.is_empty()).unwrap_or(true)
                        })
                })
                .unwrap_or(true)
        {
            fail("INVALID_MODULE_SURFACES");
        }
        if let Some(version_base) = module.get("version_base") {
            for path in [
                "/previous_active_version",
                "/new_active_version",
                "/base_artifact_hash",
                "/base_source_commit",
            ] {
                record_str(version_base, path, "module-version-base");
            }
            if version_base
                .get("previous_active_version")
                .and_then(Value::as_str)
                == version_base
                    .get("new_active_version")
                    .and_then(Value::as_str)
            {
                fail(format!("INVALID_MODULE_VERSION_BASE:{}", module_id));
            }
        }
        let build = module
            .get("build")
            .and_then(Value::as_object)
            .unwrap_or_else(|| fail(format!("INVALID_MODULE_CONTRACT:{}", module_id)));
        if build
            .get("program")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
            || build
                .get("working_directory")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
            || build
                .get("args")
                .and_then(Value::as_array)
                .map(|values| values.iter().any(|value| value.as_str().is_none()))
                .unwrap_or(true)
        {
            fail(format!("INVALID_MODULE_BUILD_CONTRACT:{}", module_id));
        }
        let regression = module
            .get("regression")
            .and_then(Value::as_object)
            .unwrap_or_else(|| fail(format!("INVALID_REGRESSION_CONTRACT:{}", module_id)));
        let required_before_freeze = regression
            .get("required_before_freeze")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| fail(format!("INVALID_REGRESSION_CONTRACT:{}", module_id)));
        if (!required_before_freeze
            && matches!(stage, "architecture_stable" | "frozen" | "retired"))
            || regression
                .get("suite_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
            || regression
                .get("input_paths")
                .and_then(Value::as_array)
                .map(|values| {
                    values.is_empty()
                        || values
                            .iter()
                            .any(|value| value.as_str().filter(|path| !path.is_empty()).is_none())
                })
                .unwrap_or(true)
            || regression
                .get("minimum_test_count")
                .and_then(Value::as_u64)
                .filter(|count| *count > 0)
                .is_none()
            || regression
                .get("allow_skipped")
                .and_then(Value::as_bool)
                .is_none()
            || regression
                .get("ordinary_mode_after_freeze")
                .and_then(Value::as_str)
                != Some("disabled")
            || regression
                .get("reenable_on")
                .and_then(Value::as_array)
                .map(|values| {
                    [
                        "source_change",
                        "contract_change",
                        "public_api_change",
                        "artifact_change",
                        "dependency_change",
                    ]
                    .iter()
                    .any(|required| !values.iter().any(|value| value.as_str() == Some(*required)))
                })
                .unwrap_or(true)
        {
            fail(format!("INVALID_REGRESSION_CONTRACT:{}", module_id));
        }
        let command = regression
            .get("command")
            .and_then(Value::as_object)
            .unwrap_or_else(|| fail(format!("INVALID_REGRESSION_CONTRACT:{}", module_id)));
        if command
            .get("program")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
            || command
                .get("working_directory")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
            || command
                .get("args")
                .and_then(Value::as_array)
                .map(|values| values.iter().any(|value| value.as_str().is_none()))
                .unwrap_or(true)
        {
            fail(format!("INVALID_REGRESSION_CONTRACT:{}", module_id));
        }
    }
    let project_id = project
        .get("project_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert_identifier(project_id, "INVALID_PROJECT_ID");
    let stage = required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT");
    if !matches!(
        stage,
        "draft"
            | "source_implemented"
            | "contract_bound"
            | "compiled"
            | "controlled_verified"
            | "architecture_stable"
            | "frozen"
            | "retired"
    ) {
        fail(format!("UNKNOWN_PROJECT_STAGE:{}", stage));
    }
    assert_sdk_lock(
        root,
        &project,
        matches!(
            stage,
            "compiled" | "controlled_verified" | "architecture_stable" | "frozen" | "retired"
        ),
    );
    assert_sdk_resources(
        root,
        matches!(
            stage,
            "compiled" | "controlled_verified" | "architecture_stable" | "frozen" | "retired"
        ),
    );
    assert_goal_contract(root, false);
    let lock_file = root.join(".appsdk").join("sdk.lock");
    let lock: Value = serde_json::from_str(
        &fs::read_to_string(&lock_file).unwrap_or_else(|_| fail("MISSING_SDK_LOCK")),
    )
    .unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
    if lock.get("sdk").and_then(Value::as_str) != Some("appsdk")
        || lock.get("version").and_then(Value::as_str)
            != project.pointer("/sdk/version").and_then(Value::as_str)
    {
        fail("INVALID_SDK_LOCK");
    }
    let artifact_file = generated_root(root, &project).join("project.compiled.json");
    let stage = required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT");
    if !admission
        && matches!(
            stage,
            "compiled" | "controlled_verified" | "architecture_stable" | "frozen" | "retired"
        )
        && !artifact_file.exists()
    {
        fail("COMPILED_STAGE_REQUIRES_ARTIFACT");
    }
    if artifact_file.exists() {
        let artifact = read_compiled_artifact(root, &project);
        assert_artifact_matches(&project, &artifact);
    }
    let _artifact = if artifact_file.exists() {
        Some(read_compiled_artifact(root, &project))
    } else {
        None
    };
    for module in project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"))
    {
        if !admission && module.get("stage").and_then(Value::as_str) == Some("frozen") {
            let id = module
                .get("module_id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT"));
            let freeze_name = freeze_record_name(id);
            let version = read_record(root, &freeze_name);
            let active_version = record_str(&version, "/active_version", &freeze_name);
            let active_root = contract_root(root, &project, "/governance/active_root");
            let active_path = active_root.join(id).join(active_version);
            if !active_path.is_dir() {
                fail("ACTIVE_ARTIFACT_MISSING");
            }
            {
                assert_no_symlink_components(root, &active_path, "active_verified");
                let active_index = active_root.join(id).join("current.json");
                if fs::symlink_metadata(&active_index)
                    .map(|metadata| metadata.file_type().is_symlink())
                    .unwrap_or(false)
                    || !active_index.is_file()
                {
                    fail("ACTIVE_INDEX_MISSING");
                }
                let index_value: Value = serde_json::from_str(
                    &fs::read_to_string(&active_index)
                        .unwrap_or_else(|_| fail("ACTIVE_INDEX_MISSING")),
                )
                .unwrap_or_else(|_| fail("INVALID_ACTIVE_INDEX"));
                if index_value.get("module_id").and_then(Value::as_str) != Some(id)
                    || index_value.get("version").and_then(Value::as_str) != Some(active_version)
                    || index_value.get("artifact_hash").and_then(Value::as_str)
                        != Some(record_str(&version, "/library_hash", &freeze_name))
                {
                    fail("ACTIVE_INDEX_MISMATCH");
                }
                let active_artifact = active_path.join("artifact.json");
                if fs::symlink_metadata(&active_artifact)
                    .map(|metadata| metadata.file_type().is_symlink())
                    .unwrap_or(false)
                    || !active_artifact.is_file()
                {
                    fail("ACTIVE_ARTIFACT_MISSING");
                }
                let active_value: Value = serde_json::from_str(
                    &fs::read_to_string(active_artifact)
                        .unwrap_or_else(|_| fail("ACTIVE_ARTIFACT_MISSING")),
                )
                .unwrap_or_else(|_| fail("INVALID_ACTIVE_ARTIFACT"));
                module_artifact_matches_project(module, &active_value);
                let generated_module = read_module_artifact(root, &project, id);
                module_artifact_matches_project(module, &generated_module);
                let protected_archive = contract_root(root, &project, "/governance/protected_root")
                    .join("history")
                    .join(id);
                if !protected_archive.is_dir() {
                    fail("PROTECTED_HISTORY_MISSING");
                }
                assert_protected_not_ignored(root, &protected_archive);
                assert_protected_archive_matches(
                    root,
                    module,
                    &generated_module,
                    &protected_archive,
                );
                if record_str(&active_value, "/artifact_hash", "active_artifact")
                    != record_str(&generated_module, "/artifact_hash", "module-artifact")
                    || record_str(&active_value, "/artifact_hash", "active_artifact")
                        != record_str(&version, "/library_hash", &freeze_name)
                {
                    fail("ACTIVE_ARTIFACT_HASH_MISMATCH");
                }
                let active_entries = active_value
                    .get("artifacts")
                    .and_then(Value::as_array)
                    .unwrap_or_else(|| fail("INVALID_ACTIVE_ARTIFACT"));
                for entry in active_entries {
                    let relative = entry
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| fail("INVALID_ACTIVE_ARTIFACT"));
                    let expected = entry
                        .get("hash")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| fail("INVALID_ACTIVE_ARTIFACT"));
                    let active_lib = active_path.join("lib").join(relative);
                    if file_sha256(&active_lib, "active_library") != expected {
                        fail("ACTIVE_LIBRARY_HASH_MISMATCH");
                    }
                }
            }
        }
    }
    if !admission
        && project
            .get("modules")
            .and_then(Value::as_array)
            .map(|modules| {
                modules.iter().any(|module| {
                    matches!(
                        module.get("stage").and_then(Value::as_str),
                        Some("architecture_stable" | "frozen" | "retired")
                    )
                })
            })
            .unwrap_or(false)
    {
        let _artifact = read_compiled_artifact(root, &project);
        for module in project
            .get("modules")
            .and_then(Value::as_array)
            .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"))
        {
            if matches!(
                module.get("stage").and_then(Value::as_str),
                Some("architecture_stable" | "frozen" | "retired")
            ) {
                let module_id = module
                    .get("module_id")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT"));
                let module_artifact = read_module_artifact(root, &project, module_id);
                module_artifact_matches_project(module, &module_artifact);
                if module.get("stage").and_then(Value::as_str) == Some("architecture_stable") {
                    let records = root.join(".appsdk/records");
                    let effectiveness_exists = records
                        .join(module_record_name("effectiveness-record", module_id))
                        .is_file();
                    let merge_exists = records
                        .join(module_record_name("merge-record", module_id))
                        .is_file();
                    let promotion_exists = records
                        .join(module_record_name("promotion-record", module_id))
                        .is_file();
                    if promotion_exists {
                        assert_record_graph(root, Some(module_id), &module_artifact, false);
                    } else {
                        assert_fix_architecture_gate(root, module_id, &module_artifact);
                        if merge_exists {
                            assert_fix_effectiveness_gate(root, module_id);
                            assert_fix_merge_gate(root, module_id);
                        } else if effectiveness_exists {
                            assert_fix_effectiveness_gate(root, module_id);
                        }
                    }
                } else {
                    assert_record_graph(root, Some(module_id), &module_artifact, true);
                }
            }
        }
    }
    if emit_result {
        println!(
            "{{\"ok\":true,\"project_id\":\"{}\",\"stage\":\"{}\"}}",
            required_str(&project, "/project_id", "INVALID_PROJECT_ID"),
            required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT")
        );
    }
}

fn verify(root: &Path, admission: bool) {
    verify_internal(root, admission, true);
}

const APPSDK_GITIGNORE_BEGIN: &str = "# BEGIN APPSDK MANAGED";
const APPSDK_GITIGNORE_END: &str = "# END APPSDK MANAGED";
const APPSDK_GITIGNORE_BLOCK: &str =
    "# BEGIN APPSDK MANAGED\n.appsdk-control/\n.appsdk/sdk.bin\n/active/lib/\n/generated/\n# END APPSDK MANAGED\n";

fn ensure_appsdk_gitignore(root: &Path) {
    let path = root.join(".gitignore");
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:gitignore");
    }
    let mut content = fs::read_to_string(&path).unwrap_or_default();
    if let Some(begin) = content.find(APPSDK_GITIGNORE_BEGIN) {
        let end_start = begin + APPSDK_GITIGNORE_BEGIN.len();
        let end = content[end_start..]
            .find(APPSDK_GITIGNORE_END)
            .map(|offset| end_start + offset)
            .unwrap_or_else(|| fail("INVALID_APPSDK_GITIGNORE_BLOCK"));
        let end_after = end + APPSDK_GITIGNORE_END.len();
        let mut updated = String::with_capacity(content.len());
        updated.push_str(&content[..begin]);
        updated.push_str(APPSDK_GITIGNORE_BLOCK);
        let suffix = &content[end_after..];
        if !suffix.trim().is_empty() {
            updated.push_str(suffix);
        }
        if updated != content {
            fs::write(path, updated).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
        }
        return;
    }
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.is_empty() {
        content.push('\n');
    }
    content.push_str(APPSDK_GITIGNORE_BLOCK);
    fs::write(path, content).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
}

fn ensure_governance_layout(root: &Path) {
    fs::create_dir_all(root.join(".appsdk")).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    for dir in [
        "playground/experiments",
        "active/lib",
        "protected/source",
        "protected/contracts",
        "protected/history",
        "generated",
        "tests/core",
        ".appsdk/records",
        ".appsdk-control",
    ] {
        fs::create_dir_all(root.join(dir)).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    }
    bootstrap_contracts(root);
    ensure_appsdk_gitignore(root);
}

fn write_if_missing(root: &Path, relative: &str, content: &str) {
    let target = root.join(relative);
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(format!("GOVERNANCE_PATH_SYMLINK:{}", relative));
    }
    if target.exists() {
        return;
    }
    fs::write(target, content).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
}

fn write_project_scaffold(root: &Path) {
    write_if_missing(
        root,
        ".appsdk/project.json",
        r#"{
  "schema_version": 1,
  "project_id": "change-me",
  "sdk": {"name": "appsdk", "version": "0.1.6", "bundle_manifest": ".appsdk/contracts/sdk-bundle.manifest.json", "resource_record": ".appsdk/sdk-resources.json"},
  "lifecycle": {"stage": "draft"},
  "access": {"protected_paths": [".appsdk/**", "generated/**", "protected/source/**"]},
  "development_scenarios": {"manifest": ".appsdk/contracts/development-scenarios.manifest.json", "enabled": []},
  "governance": {
    "playground_root": "playground/experiments/**",
    "active_root": "active/lib/**",
    "protected_root": "protected/**",
    "generated_root": "generated/**",
    "active_kind": "immutable_consumable_library",
    "protected_kinds": ["source", "contracts", "history"],
    "generated_kinds": ["compiler_output", "indexes"],
    "freeze_requirements": ["git_clean", "source_commit_or_tag", "library_hash", "public_api_hash", "review_pass", "previous_active_immutable"],
    "promotion_requires": ["experiment_evidence", "architecture_review_pass", "unique_owner", "required_gates"],
    "runtime_forbidden_roots": ["playground/**", "generated/**"],
    "record_contracts": ["contracts/records/worktree-record.schema.json", "contracts/records/reproduction-record.schema.json", "contracts/records/evidence-record.schema.json", "contracts/records/fix-candidate-record.schema.json", "contracts/records/goal-clarification-record.schema.json", "contracts/records/review-record.schema.json", "contracts/records/effectiveness-record.schema.json", "contracts/records/pre-review-validation-record.schema.json", "contracts/records/collaboration-record.schema.json", "contracts/records/collaboration-index.schema.json", "contracts/records/merge-queue-record.schema.json", "contracts/records/merge-queue-state.schema.json", "contracts/records/integration-record.schema.json", "contracts/records/mainline-receipt-record.schema.json", "contracts/records/merge-record.schema.json", "contracts/records/promotion-record.schema.json", "contracts/records/regression-report.schema.json", "contracts/records/freeze-record.schema.json", "contracts/records/record-graph.contract.json"],
    "zone_transition_contract": "contracts/transitions/zone-transition-manifest.json",
    "playground_retention": "archive_then_remove",
    "debug_merge_comment_required": true
  },
  "lifecycles": {"issue": "open", "library": "draft", "source_snapshot": "mutable", "artifact": "generated"},
  "modules": [{"module_id":"app-core","stage":"source_implemented","owned_paths":["playground/experiments/**","protected/source/**","tests/core/**"],"source_owner":"app-core","active_artifact":"active/lib/app-core/**","generated_outputs":["generated/**"],"contract_paths":["contracts/records/**","contracts/transitions/**"],"dependency_modules":[],"build":{"program":"sh","args":["-c","mkdir -p generated/modules/app-core/lib && printf 'app-core placeholder\\n' > generated/modules/app-core/lib/app-core.placeholder"],"working_directory":"."},"artifact_paths":["app-core.placeholder"],"regression":{"required_before_freeze":true,"suite_id":"app-core-regression","command":{"program":"cargo","args":["test"],"working_directory":"."},"input_paths":["playground/experiments/**","tests/core/**"],"minimum_test_count":1,"allow_skipped":false,"ordinary_mode_after_freeze":"disabled","reenable_on":["source_change","contract_change","public_api_change","artifact_change","dependency_change"]}}]
}
"#,
    );
    write_if_missing(
        root,
        ".appsdk/goal.json",
        r#"{"goal_id":"goal-change-me","raw_request":"Describe the intended change before implementation.","understood_objective":"The objective will be restated and confirmed before admission.","acceptance_criteria":["The user-confirmed acceptance criteria are recorded before implementation."],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"received","confirmed_by":null,"confirmed_at":null,"created_at":"2026-01-01T00:00:00Z"}
"#,
    );
    write_if_missing(
        root,
        ".appsdk/sdk.lock",
        r#"{"sdk":"appsdk","version":"0.1.6","digest":"sha256:replace-with-compiled-sdk-digest","compiler_digest":"sha256:replace-with-compiler-digest","bundle_digest":"sha256:replace-with-sdk-bundle-digest","bundle_manifest_digest":"sha256:replace-with-bundle-manifest-digest","contract_schema":1}
"#,
    );
}

fn assert_init_workspace_safe(workspace: &Path) {
    if fs::symlink_metadata(workspace)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(format!("TARGET_SYMLINK:{}", workspace.display()));
    }
    if workspace.exists() && !workspace.is_dir() {
        fail(format!("TARGET_NOT_DIRECTORY:{}", workspace.display()));
    }
    for ancestor in workspace.ancestors() {
        if ancestor == Path::new("/tmp") || ancestor == Path::new("/var") {
            continue;
        }
        if fs::symlink_metadata(ancestor)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(format!("TARGET_PARENT_SYMLINK:{}", ancestor.display()));
        }
    }
}

fn resolve_init_target(workspace: &Path, project_root: Option<&str>) -> PathBuf {
    assert_init_workspace_safe(workspace);
    let Some(project_root) = project_root else {
        return workspace.to_path_buf();
    };
    let relative = Path::new(project_root);
    if relative == Path::new(".") {
        return workspace.to_path_buf();
    }
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        fail("INVALID_PROJECT_ROOT");
    }
    workspace.join(relative)
}

fn preparation_file(workspace: &Path) -> PathBuf {
    workspace.join(".appsdk-prepare.json")
}

fn preparation_exists(workspace: &Path) -> bool {
    let path = preparation_file(workspace);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                fail("PREPARATION_SYMLINK");
            }
            true
        }
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(_) => fail("PREPARATION_INVALID"),
    }
}

fn preparation_template() -> &'static str {
    r#"{
  "schema_version": 1,
  "preparation_id": "prepare-change-me",
  "status": "draft",
  "objective": "Describe the confirmed project or module change.",
  "change_kind": null,
  "project_root": null,
  "legacy_roots": [],
  "new_roots": [],
  "protected_roots": [],
  "runtime_forbidden_roots": [],
  "boundary": {
    "allowed_paths": [],
    "forbidden_paths": [],
    "payload_control_separation": "must be confirmed"
  },
  "acceptance_criteria": [],
  "non_goals": [],
  "assumptions": [],
  "questions": [
    {"question_id":"scope-kind","question":"Is this a new project, module refactor, project refactor, or debug task?","status":"open"},
    {"question_id":"project-root","question":"Which relative directory is the new AppSDK project root?","status":"open"},
    {"question_id":"legacy-boundary","question":"Which existing directories remain read-only and outside the new project?","status":"open"},
    {"question_id":"new-boundary","question":"Which directories may the new project create or modify?","status":"open"}
  ],
  "confirmed_by": null,
  "confirmed_at": null,
  "created_at": "2026-01-01T00:00:00Z"
}
"#
}

fn read_preparation(workspace: &Path) -> Value {
    let path = preparation_file(workspace);
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("PREPARATION_SYMLINK");
    }
    let text = fs::read_to_string(path).unwrap_or_else(|_| fail("PREPARATION_MISSING"));
    let value: Value = serde_json::from_str(&text).unwrap_or_else(|_| fail("PREPARATION_INVALID"));
    if value.get("schema_version").and_then(Value::as_u64) != Some(1)
        || value.get("status").and_then(Value::as_str) != Some("confirmed")
        || value
            .get("objective")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .is_none()
        || value
            .get("change_kind")
            .and_then(Value::as_str)
            .filter(|value| {
                matches!(
                    *value,
                    "new_project" | "module_refactor" | "project_refactor" | "debug"
                )
            })
            .is_none()
        || value.get("project_root").and_then(Value::as_str).is_none()
        || value
            .get("confirmed_by")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .is_none()
        || value.get("confirmed_at").and_then(Value::as_str).is_none()
    {
        fail("PREPARATION_NOT_CONFIRMED");
    }
    value
}

fn read_init_preparation(workspace: &Path) -> (Value, PathBuf) {
    for preparation_workspace in workspace.ancestors() {
        assert_init_workspace_safe(preparation_workspace);
        if !preparation_exists(preparation_workspace) {
            continue;
        }
        let preparation = read_preparation(preparation_workspace);
        if preparation_workspace == workspace {
            return (preparation, preparation_workspace.to_path_buf());
        }
        let project_root = preparation
            .get("project_root")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("PREPARATION_PROJECT_ROOT_MISSING"));
        if resolve_init_target(preparation_workspace, Some(project_root)) != workspace {
            fail("PREPARATION_PROJECT_ROOT_MISMATCH");
        }
        return (preparation, preparation_workspace.to_path_buf());
    }
    fail("PREPARATION_MISSING")
}

fn prepare_project(workspace: &Path) {
    assert_init_workspace_safe(workspace);
    fs::create_dir_all(workspace).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    let path = preparation_file(workspace);
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("PREPARATION_SYMLINK");
    }
    if !path.exists() {
        fs::write(&path, preparation_template())
            .unwrap_or_else(|_| fail("PREPARATION_WRITE_FAILED"));
        println!("created {}", path.display());
    } else {
        let text = fs::read_to_string(&path).unwrap_or_else(|_| fail("PREPARATION_INVALID"));
        println!("{}", text);
    }
}

fn init_project(root: &Path) {
    if root.exists()
        && fs::symlink_metadata(root)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    {
        fail(format!("TARGET_SYMLINK:{}", root.display()));
    }
    fs::create_dir_all(root).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    ensure_governance_layout(root);
    write_project_scaffold(root);
    install_bundle_resources(root);
    println!("initialized {}", root.display());
}

fn new_project(root: &Path) {
    if root.exists() {
        if fs::symlink_metadata(root)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(format!("TARGET_SYMLINK:{}", root.display()));
        }
        if fs::read_dir(root)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(true)
        {
            fail(format!("TARGET_NOT_EMPTY:{}", root.display()));
        }
    }
    let mut parent = root.parent().unwrap_or(root).to_path_buf();
    while !parent.exists() {
        let next = parent.parent().unwrap_or(&parent).to_path_buf();
        if next == parent {
            break;
        }
        parent = next;
    }
    for ancestor in parent.ancestors() {
        if ancestor == Path::new("/tmp") || ancestor == Path::new("/var") {
            continue;
        }
        if fs::symlink_metadata(ancestor)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(format!("TARGET_PARENT_SYMLINK:{}", ancestor.display()));
        }
    }
    ensure_governance_layout(root);
    write_project_scaffold(root);
    install_bundle_resources(root);
    println!("created {}", root.display());
}

fn sdk_map_migration_root(root: &Path) -> PathBuf {
    root.join(".appsdk")
        .join("migrations")
        .join("0.1.5-to-0.1.6")
}

fn sdk_map_migration_entry<'a>(manifest: &'a Value, name: &str) -> &'a Value {
    manifest
        .get("maps")
        .and_then(Value::as_array)
        .and_then(|maps| {
            maps.iter()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
        })
        .unwrap_or_else(|| fail("INVALID_SDK_MAP_MIGRATION_MANIFEST"))
}

fn assert_sdk_migration_record(root: &Path) -> Option<Value> {
    let migration_root = sdk_map_migration_root(root);
    let record_path = migration_root.join("record.json");
    if !record_path.exists() {
        if migration_root.exists() {
            fail("SDK_MIGRATION_RECORD_MISSING");
        }
        return None;
    }
    assert_no_symlink_components(root, &migration_root, "sdk_migration");
    let record: Value = serde_json::from_str(
        &fs::read_to_string(&record_path).unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD")),
    )
    .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD"));
    if record.get("schema_version").and_then(Value::as_u64) != Some(1)
        || record.get("migration_id").and_then(Value::as_str) != Some("appsdk-0.1.5-to-0.1.6")
        || record.get("source_version").and_then(Value::as_str) != Some("0.1.5")
        || record.get("target_version").and_then(Value::as_str) != Some("0.1.6")
        || record.get("bundle_digest").and_then(Value::as_str) != Some(sdk_bundle_digest().as_str())
        || DateTime::parse_from_rfc3339(record_str(&record, "/created_at", "sdk-migration-record"))
            .is_err()
    {
        fail("INVALID_SDK_MIGRATION_RECORD");
    }
    let manifest = sdk_map_migration_manifest();
    let maps = record
        .get("maps")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
    if maps.len() != GOVERNANCE_MAP_NAMES.len() {
        fail("INVALID_SDK_MIGRATION_RECORD");
    }
    for name in GOVERNANCE_MAP_NAMES {
        let declared = sdk_map_migration_entry(&manifest, name);
        let entry = maps
            .iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        let expected_snapshot = format!(".appsdk/migrations/0.1.5-to-0.1.6/maps/{}", name);
        let canonical_source = entry
            .get("canonical_source_digest")
            .unwrap_or_else(|| entry.get("source_digest").unwrap());
        let canonical_target = entry
            .get("canonical_target_digest")
            .unwrap_or_else(|| entry.get("target_digest").unwrap());
        if (Some(canonical_source) != declared.get("source_digest")
            && entry
                .get("canonical_source_digest")
                .is_some_and(|value| !value.is_null()))
            || (Some(canonical_target) != declared.get("target_digest")
                && entry
                    .get("canonical_target_digest")
                    .is_some_and(|value| !value.is_null()))
            || entry.get("snapshot_path").and_then(Value::as_str)
                != Some(expected_snapshot.as_str())
        {
            fail("INVALID_SDK_MIGRATION_RECORD");
        }
        let snapshot = root.join(&expected_snapshot);
        if !snapshot.is_file()
            || file_sha256(&snapshot, "sdk_migration_snapshot")
                != record_str(entry, "/source_digest", "sdk-migration-map")
        {
            fail(format!("SDK_MIGRATION_SNAPSHOT_MISMATCH:{}", name));
        }
        if file_sha256(&root.join(".appsdk/maps").join(name), "governance_map")
            != record_str(entry, "/target_digest", "sdk-migration-map")
        {
            fail(format!("SDK_MIGRATION_TARGET_MAP_MISMATCH:{}", name));
        }
    }
    let reviews = record
        .get("frozen_reviews")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
    let mut modules = std::collections::HashSet::new();
    for review in reviews {
        let module_id = record_str(review, "/module_id", "sdk-migration-review");
        assert_identifier(module_id, "INVALID_SDK_MIGRATION_RECORD");
        if !modules.insert(module_id)
            || record_str(review, "/review_id", "sdk-migration-review").is_empty()
        {
            fail("INVALID_SDK_MIGRATION_RECORD");
        }
    }
    if let Some(legacy_reviews) = record.get("legacy_reconciled_reviews") {
        let legacy_reviews = legacy_reviews
            .as_array()
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        for review in legacy_reviews {
            let module_id = record_str(review, "/module_id", "sdk-migration-review");
            assert_identifier(module_id, "INVALID_SDK_MIGRATION_RECORD");
            if record_str(review, "/review_id", "sdk-migration-review").is_empty()
                || record_str(review, "/stage", "sdk-migration-review") != "source_implemented"
                || modules.contains(module_id)
            {
                fail("INVALID_SDK_MIGRATION_RECORD");
            }
            modules.insert(module_id);
        }
    }
    Some(record)
}

fn install_current_governance_maps(root: &Path) {
    let record_path = sdk_map_migration_root(root).join("record.json");
    if record_path.is_file() {
        let record: Value = serde_json::from_str(
            &fs::read_to_string(&record_path)
                .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD")),
        )
        .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD"));
        if record
            .pointer("/maps/0/canonical_source_digest")
            .is_some_and(Value::is_string)
        {
            return;
        }
    }
    let manifest = sdk_map_migration_manifest();
    for name in GOVERNANCE_MAP_NAMES {
        let target = root.join(".appsdk/maps").join(name);
        atomic_write_bytes(
            &target,
            canonical_governance_map(name).as_bytes(),
            "SDK_MAP_MIGRATION_WRITE_FAILED",
        );
        if file_sha256(&target, "governance_map")
            != record_str(
                sdk_map_migration_entry(&manifest, name),
                "/target_digest",
                "sdk-map-migration",
            )
        {
            fail(format!("SDK_MAP_MIGRATION_TARGET_MISMATCH:{}", name));
        }
    }
}

fn migrate_governance_maps(root: &Path, project: &Value, project_version: &str) {
    let migration_root = sdk_map_migration_root(root);
    if migration_root.join("record.json").is_file() {
        install_current_governance_maps(root);
        assert_sdk_migration_record(root);
        return;
    }
    let manifest = sdk_map_migration_manifest();
    let canonical_source_matches = GOVERNANCE_MAP_NAMES.iter().all(|name| {
        file_sha256(&root.join(".appsdk/maps").join(name), "governance_map")
            == record_str(
                sdk_map_migration_entry(&manifest, name),
                "/source_digest",
                "sdk-map-migration",
            )
    });
    for name in GOVERNANCE_MAP_NAMES {
        let live = root.join(".appsdk/maps").join(name);
        if !live.is_file() {
            fail(format!("MISSING_GOVERNANCE_MAP:{}", name));
        }
        let _ = file_sha256(&live, "governance_map");
    }
    if project_version == "0.1.6" && !canonical_source_matches {
        return;
    }

    let mut frozen_reviews = Vec::new();
    let mut legacy_reconciled_reviews = Vec::new();
    for module in project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"))
    {
        let module_id = record_str(module, "/module_id", "module");
        let review_name = module_record_name("review-record", module_id);
        let review_path = root.join(".appsdk/records").join(&review_name);
        if !review_path.exists() {
            continue;
        }
        let stage = module
            .get("stage")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail(format!("INVALID_MODULE_CONTRACT:{}", module_id)));
        let review = read_record(root, &review_name);
        if !matches!(stage, "frozen" | "retired") {
            if stage != "source_implemented"
                || review.get("verdict").and_then(Value::as_str) != Some("PASS")
            {
                fail(format!("SDK_MIGRATION_OPEN_REVIEW:{}", module_id));
            }
            legacy_reconciled_reviews.push(serde_json::json!({
                "module_id": module_id,
                "review_id": record_str(&review, "/review_id", &review_name),
                "stage": stage
            }));
        }
        for name in GOVERNANCE_MAP_NAMES {
            let entry = sdk_map_migration_entry(&manifest, name);
            let field = record_str(entry, "/review_hash_field", "sdk-map-migration");
            let review_hash = record_str(&review, &format!("/{}", field), &review_name);
            if canonical_source_matches
                && review_hash != record_str(entry, "/source_digest", "sdk-map-migration")
            {
                fail(format!(
                    "SDK_MIGRATION_FROZEN_REVIEW_MAP_MISMATCH:{}:{}",
                    module_id, name
                ));
            }
        }
        frozen_reviews.push(serde_json::json!({
            "module_id": module_id,
            "review_id": record_str(&review, "/review_id", &review_name)
        }));
    }

    let migrations = root.join(".appsdk/migrations");
    fs::create_dir_all(&migrations).unwrap_or_else(|_| fail("SDK_MAP_MIGRATION_WRITE_FAILED"));
    let staging = migrations.join(".0.1.5-to-0.1.6.staging");
    if staging.exists() {
        assert_no_symlink_components(root, &staging, "sdk_map_migration_staging");
        fs::remove_dir_all(&staging)
            .unwrap_or_else(|_| fail("SDK_MAP_MIGRATION_STAGING_CLEANUP_FAILED"));
    }
    fs::create_dir_all(staging.join("maps"))
        .unwrap_or_else(|_| fail("SDK_MAP_MIGRATION_WRITE_FAILED"));
    let mut map_records = Vec::new();
    for name in GOVERNANCE_MAP_NAMES {
        let entry = sdk_map_migration_entry(&manifest, name);
        let snapshot = staging.join("maps").join(name);
        atomic_write_bytes(
            &snapshot,
            &fs::read(root.join(".appsdk/maps").join(name))
                .unwrap_or_else(|_| fail("SDK_MAP_MIGRATION_SOURCE_READ_FAILED")),
            "SDK_MAP_MIGRATION_WRITE_FAILED",
        );
        let project_map_hash = file_sha256(&root.join(".appsdk/maps").join(name), "governance_map");
        map_records.push(serde_json::json!({
            "name": name,
            "source_digest": if canonical_source_matches { entry["source_digest"].clone() } else { Value::String(project_map_hash.clone()) },
            "target_digest": if canonical_source_matches { entry["target_digest"].clone() } else { Value::String(project_map_hash) },
            "canonical_source_digest": if canonical_source_matches { Value::Null } else { entry["source_digest"].clone() },
            "canonical_target_digest": if canonical_source_matches { Value::Null } else { entry["target_digest"].clone() },
            "snapshot_path": format!(
                ".appsdk/migrations/0.1.5-to-0.1.6/maps/{}",
                name
            )
        }));
    }
    atomic_write_json(
        &staging.join("record.json"),
        &serde_json::json!({
            "schema_version": 1,
            "migration_id": "appsdk-0.1.5-to-0.1.6",
            "source_version": "0.1.5",
            "target_version": "0.1.6",
            "bundle_digest": sdk_bundle_digest(),
            "maps": map_records,
            "frozen_reviews": frozen_reviews,
            "legacy_reconciled_reviews": legacy_reconciled_reviews,
            "created_at": Utc::now().to_rfc3339()
        }),
        "SDK_MAP_MIGRATION_WRITE_FAILED",
    );
    if migration_root.exists() {
        fail("SDK_MIGRATION_RECORD_EXISTS");
    }
    fs::rename(&staging, &migration_root)
        .unwrap_or_else(|_| fail("SDK_MAP_MIGRATION_WRITE_FAILED"));
    install_current_governance_maps(root);
    let _ = assert_sdk_migration_record(root);
    assert_governance_maps(root);
}

fn pin_lock(root: &Path, binary: &Path) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_no_symlink_components(root, &root.join(".appsdk"), "appsdk_control");
    let mut project = read_project(root);
    let project_version = required_str(&project, "/sdk/version", "INVALID_SDK_CONTRACT");
    if !matches!(project_version, "0.1.5" | "0.1.6") {
        fail(format!(
            "UNSUPPORTED_SDK_MIGRATION:{}:0.1.6",
            project_version
        ));
    }
    let binary = binary
        .canonicalize()
        .unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
    let bytes = fs::read(&binary).unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
    let digest = digest_bytes(&bytes);
    let running_binary = env::current_exe().unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
    if digest_bytes(&fs::read(running_binary).unwrap_or_else(|_| fail("SDK_BINARY_MISSING")))
        != digest
    {
        fail("SDK_PIN_BINARY_BUNDLE_MISMATCH");
    }
    migrate_governance_maps(root, &project, project_version);
    project["sdk"]["version"] = Value::String("0.1.6".into());
    let mut lock = serde_json::Map::new();
    lock.insert("sdk".into(), Value::String("appsdk".into()));
    lock.insert("version".into(), Value::String("0.1.6".into()));
    lock.insert("digest".into(), Value::String(digest.clone()));
    lock.insert("compiler_digest".into(), Value::String(digest));
    lock.insert("bundle_digest".into(), Value::String(sdk_bundle_digest()));
    lock.insert(
        "bundle_manifest_digest".into(),
        Value::String(digest_bytes(SDK_BUNDLE_MANIFEST.as_bytes())),
    );
    lock.insert(
        "bundle_resources".into(),
        serde_json::from_str::<Value>(SDK_BUNDLE_MANIFEST)
            .unwrap_or_else(|_| fail("INVALID_SDK_BUNDLE"))
            .get("resources")
            .cloned()
            .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE")),
    );
    let pinned_binary = root.join(".appsdk/sdk.bin");
    if fs::symlink_metadata(&pinned_binary)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_binary");
    }
    atomic_write_bytes(&pinned_binary, &bytes, "SDK_BINARY_WRITE_FAILED");
    install_bundle_resources(root);
    lock.insert("binary_ref".into(), Value::String("project-sdk".into()));
    lock.insert(
        "contract_schema".into(),
        project
            .get("schema_version")
            .cloned()
            .unwrap_or_else(|| fail("UNSUPPORTED_PROJECT_SCHEMA")),
    );
    let lock_path = root.join(".appsdk/sdk.lock");
    if fs::symlink_metadata(&lock_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_lock");
    }
    atomic_write_json(&lock_path, &Value::Object(lock), "SDK_LOCK_WRITE_FAILED");
    write_project(root, &project);
    println!("pinned {}", binary.display());
}

fn main() {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("version") => println!("appsdk 0.1.6 (rust)"),
        Some("verify-sdk-source-registry") => assert_sdk_source_registry(Path::new(
            &args.next().unwrap_or_else(|| ".".into()),
        )),
        Some("verify") => {
            let first = args.next().unwrap_or_else(|| ".".into());
            if first == "--admission" {
                let root = args
                    .next()
                    .unwrap_or_else(|| fail("USAGE: appsdk verify [--admission] <dir>"));
                verify(Path::new(&root), true);
            } else if first == "--review-admission" {
                let root = args.next().unwrap_or_else(|| {
                    fail("USAGE: appsdk verify --review-admission <dir> --module <id>")
                });
                if args.next().as_deref() != Some("--module") {
                    fail("USAGE: appsdk verify --review-admission <dir> --module <id>");
                }
                let module_id = args.next().unwrap_or_else(|| {
                    fail("USAGE: appsdk verify --review-admission <dir> --module <id>")
                });
                if args.next().is_some() {
                    fail("USAGE: appsdk verify --review-admission <dir> --module <id>");
                }
                verify_review_admission(Path::new(&root), &module_id);
            } else {
                verify(Path::new(&first), false);
            }
        }
        Some("pin-lock") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk pin-lock <dir> --binary <path>")));
            if args.next().as_deref() != Some("--binary") { fail("USAGE: appsdk pin-lock <dir> --binary <path>"); }
            let binary = args.next().unwrap_or_else(|| fail("USAGE: appsdk pin-lock <dir> --binary <path>"));
            pin_lock(&root, Path::new(&binary));
        }
        Some("compile") => compile(Path::new(&args.next().unwrap_or_else(|| ".".into()))),
        Some("compile-module") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk compile-module <dir> --module <id>")));
            if args.next().as_deref() != Some("--module") { fail("USAGE: appsdk compile-module <dir> --module <id>"); }
            compile_module(&root, &args.next().unwrap_or_else(|| fail("USAGE: appsdk compile-module <dir> --module <id>")));
        }
        Some("begin-version") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk begin-version <dir> --module <id> --from <version> --to <version>")));
            if args.next().as_deref() != Some("--module") { fail("USAGE: appsdk begin-version <dir> --module <id> --from <version> --to <version>"); }
            let module_id = args.next().unwrap_or_else(|| fail("USAGE: appsdk begin-version <dir> --module <id> --from <version> --to <version>"));
            if args.next().as_deref() != Some("--from") { fail("USAGE: appsdk begin-version <dir> --module <id> --from <version> --to <version>"); }
            let from = args.next().unwrap_or_else(|| fail("USAGE: appsdk begin-version <dir> --module <id> --from <version> --to <version>"));
            if args.next().as_deref() != Some("--to") { fail("USAGE: appsdk begin-version <dir> --module <id> --from <version> --to <version>"); }
            let to = args.next().unwrap_or_else(|| fail("USAGE: appsdk begin-version <dir> --module <id> --from <version> --to <version>"));
            if args.next().is_some() { fail("USAGE: appsdk begin-version <dir> --module <id> --from <version> --to <version>"); }
            begin_version(&root, &module_id, &from, &to);
        }
        Some("rehydrate-frozen") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk rehydrate-frozen <dir> --module <id>")));
            if args.next().as_deref() != Some("--module") { fail("USAGE: appsdk rehydrate-frozen <dir> --module <id>"); }
            let module_id = args.next().unwrap_or_else(|| fail("USAGE: appsdk rehydrate-frozen <dir> --module <id>"));
            if args.next().is_some() { fail("USAGE: appsdk rehydrate-frozen <dir> --module <id>"); }
            rehydrate_frozen(&root, &module_id);
        }
        Some("promote") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk promote <dir> --to <stage>")));
            if args.next().as_deref() != Some("--to") { fail("USAGE: appsdk promote <dir> --to <stage>"); }
            promote(&root, &args.next().unwrap_or_else(|| fail("USAGE: appsdk promote <dir> --to <stage>")));
        }
        Some("promote-module") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk promote-module <dir> --module <id> --to <stage>")));
            if args.next().as_deref() != Some("--module") { fail("USAGE: appsdk promote-module <dir> --module <id> --to <stage>"); }
            let module_id = args.next().unwrap_or_else(|| fail("USAGE: appsdk promote-module <dir> --module <id> --to <stage>"));
            if args.next().as_deref() != Some("--to") { fail("USAGE: appsdk promote-module <dir> --module <id> --to <stage>"); }
            let target = args.next().unwrap_or_else(|| fail("USAGE: appsdk promote-module <dir> --module <id> --to <stage>"));
            promote_module(&root, &module_id, &target);
        }
        Some("freeze") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk freeze <dir> --module <id>")));
            if args.next().as_deref() != Some("--module") { fail("USAGE: appsdk freeze <dir> --module <id>"); }
            freeze_module(&root, &args.next().unwrap_or_else(|| fail("USAGE: appsdk freeze <dir> --module <id>")));
        }
        Some("publish-active") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk publish-active <dir> --module <id> --version <version>")));
            if args.next().as_deref() != Some("--module") { fail("USAGE: appsdk publish-active <dir> --module <id> --version <version>"); }
            let module_id = args.next().unwrap_or_else(|| fail("USAGE: appsdk publish-active <dir> --module <id> --version <version>"));
            if args.next().as_deref() != Some("--version") { fail("USAGE: appsdk publish-active <dir> --module <id> --version <version>"); }
            let version = args.next().unwrap_or_else(|| fail("USAGE: appsdk publish-active <dir> --module <id> --version <version>"));
            publish_active(&root, &module_id, &version);
        }
        Some("new") => {
            let root = args.next().unwrap_or_else(|| fail("USAGE: appsdk new <dir>"));
            if args.next().is_some() {
                fail("USAGE: appsdk new <dir>");
            }
            new_project(Path::new(&root));
        }
        Some("init") => {
            let workspace =
                args.next()
                    .unwrap_or_else(|| fail("USAGE: appsdk init <workspace> [--project-root <relative-path>]"));
            let project_root = match args.next().as_deref() {
                None => None,
                Some("--project-root") => Some(
                    args.next()
                        .unwrap_or_else(|| fail("USAGE: appsdk init <workspace> --project-root <relative-path>")),
                ),
                Some(_) => fail("USAGE: appsdk init <workspace> [--project-root <relative-path>]"),
            };
            if args.next().is_some() {
                fail("USAGE: appsdk init <workspace> [--project-root <relative-path>]");
            }
            let workspace_path = Path::new(&workspace);
            let (preparation, preparation_workspace) = read_init_preparation(workspace_path);
            let prepared_root = preparation
                .get("project_root")
                .and_then(Value::as_str)
                .unwrap_or_else(|| fail("PREPARATION_PROJECT_ROOT_MISSING"));
            if let Some(explicit_root) = project_root.as_deref() {
                let _ = resolve_init_target(&preparation_workspace, Some(explicit_root));
            }
            if project_root.as_deref().is_some_and(|root| root != prepared_root) {
                fail("PREPARATION_PROJECT_ROOT_MISMATCH");
            }
            let root = resolve_init_target(&preparation_workspace, Some(prepared_root));
            init_project(&root);
        }
        Some("prepare") => {
            let workspace = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk prepare <workspace>"));
            if args.next().is_some() {
                fail("USAGE: appsdk prepare <workspace>");
            }
            prepare_project(Path::new(&workspace));
        }
        _ => fail("USAGE: appsdk version | prepare <workspace> | init <workspace> [--project-root <relative-path>] | new <dir> | verify <dir> | pin-lock <dir> --binary <path> | compile <dir> | compile-module <dir> --module <id> | begin-version <dir> --module <id> --from <version> --to <version> | rehydrate-frozen <dir> --module <id> | promote <dir> --to <stage> | promote-module <dir> --module <id> --to <stage> | freeze <dir> --module <id> | publish-active <dir> --module <id> --version <version>"),
    }
}

#[allow(dead_code)]
fn digest_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}
