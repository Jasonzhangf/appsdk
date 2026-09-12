use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::raw::c_int;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod guidance;
mod long_horizon_policy;
mod long_horizon_role;
mod memory;

use long_horizon_policy::{generate_long_horizon_master_prompt, ExecutionRole, POLICY};
use long_horizon_role::execution_role;

mod communication;

const SDK_BUNDLE_MANIFEST: &str = include_str!("../../contracts/sdk-bundle.manifest.json");
const SDK_MAP_MIGRATION_MANIFEST: &str =
    include_str!("../../contracts/migrations/sdk-0.1.5-to-0.1.6.json");
const PROJECT_AGENTS_TEMPLATE: &str = include_str!("../../templates/minimal/AGENTS.md");
const CANONICAL_ZONE_TRANSITION_CONTRACT: &str =
    include_str!("../../contracts/transitions/zone-transition.manifest.json");
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
        "contracts/communication/communication-request.schema.json",
        "contracts",
        include_str!("../../contracts/communication/communication-request.schema.json"),
    ),
    (
        "contracts/communication/communication-event.schema.json",
        "contracts",
        include_str!("../../contracts/communication/communication-event.schema.json"),
    ),
    (
        "contracts/communication/communication-capabilities.schema.json",
        "contracts",
        include_str!("../../contracts/communication/communication-capabilities.schema.json"),
    ),
    (
        "contracts/development-scenarios.manifest.json",
        "contracts",
        include_str!("../../contracts/development-scenarios.manifest.json"),
    ),
    (
        "contracts/guidance/guidance-manifest.schema.json",
        "contracts",
        include_str!("../../contracts/guidance/guidance-manifest.schema.json"),
    ),
    (
        "contracts/guidance/tour-review.schema.json",
        "contracts",
        include_str!("../../contracts/guidance/tour-review.schema.json"),
    ),
    (
        "contracts/memory/memory-entry.schema.json",
        "contracts",
        include_str!("../../contracts/memory/memory-entry.schema.json"),
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
        CANONICAL_ZONE_TRANSITION_CONTRACT,
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
        "contracts/records/goal-clarification-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/goal-clarification-record.schema.json"),
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
        "contracts/records/plan-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/plan-record.schema.json"),
    ),
    (
        "contracts/records/plan-revision-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/plan-revision-record.schema.json"),
    ),
    (
        "contracts/records/step-execution-record.schema.json",
        "contracts",
        include_str!("../../contracts/records/step-execution-record.schema.json"),
    ),
    (
        "docs/design/appsdk-project-integration.md",
        "docs",
        include_str!("../../docs/design/appsdk-project-integration.md"),
    ),
    (
        "docs/design/apps-sdk-communication.md",
        "docs",
        include_str!("../../docs/design/apps-sdk-communication.md"),
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
        "docs/design/appsdk-guidance-framework.md",
        "docs",
        include_str!("../../docs/design/appsdk-guidance-framework.md"),
    ),
    (
        "docs/test-design/appsdk-guidance-framework.md",
        "docs",
        include_str!("../../docs/test-design/appsdk-guidance-framework.md"),
    ),
    (
        "docs/architecture/development-process-control-harness.md",
        "docs",
        include_str!("../../docs/architecture/development-process-control-harness.md"),
    ),
    (
        "docs/architecture/appsdk-governance-architecture.md",
        "docs",
        include_str!("../../docs/architecture/appsdk-governance-architecture.md"),
    ),
    (
        "docs/design/project-memory.md",
        "docs",
        include_str!("../../docs/design/project-memory.md"),
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
    (
        "skills/appsdk-project-governance/appsdk-guidance.json",
        "skills",
        include_str!("../../skills/appsdk-project-governance/appsdk-guidance.json"),
    ),
    (
        "skills/appsdk-project-governance/agents/openai.yaml",
        "skills",
        include_str!("../../skills/appsdk-project-governance/agents/openai.yaml"),
    ),
    (
        "skills/appsdk-project-governance/references/bootstrap-migration.md",
        "skills",
        include_str!("../../skills/appsdk-project-governance/references/bootstrap-migration.md"),
    ),
    (
        "skills/appsdk-project-governance/references/contracts-and-failures.md",
        "skills",
        include_str!("../../skills/appsdk-project-governance/references/contracts-and-failures.md"),
    ),
    (
        "skills/appsdk-project-governance/references/development-debug.md",
        "skills",
        include_str!("../../skills/appsdk-project-governance/references/development-debug.md"),
    ),
    (
        "skills/appsdk-project-governance/references/goal-prompt.md",
        "skills",
        include_str!("../../skills/appsdk-project-governance/references/goal-prompt.md"),
    ),
    (
        "skills/appsdk-project-governance/references/process-control-harness.md",
        "skills",
        include_str!(
            "../../skills/appsdk-project-governance/references/process-control-harness.md"
        ),
    ),
    (
        "skills/appsdk-project-governance/references/review-delivery.md",
        "skills",
        include_str!("../../skills/appsdk-project-governance/references/review-delivery.md"),
    ),
    (
        "skills/appsdk-migration/SKILL.md",
        "skills",
        include_str!("../../skills/appsdk-migration/SKILL.md"),
    ),
    (
        "skills/project-memory/SKILL.md",
        "skills",
        include_str!("../../skills/project-memory/SKILL.md"),
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
        "skills" => format!(
            ".appsdk/skills/{}",
            source
                .strip_prefix("skills/")
                .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE"))
        ),
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

fn reconcile_authoring_bundle_manifest(root: &Path) {
    let authoring = root.join("contracts/sdk-bundle.manifest.json");
    if !authoring.exists() {
        return;
    }
    assert_no_symlink_components(root, &authoring, "sdk_authoring_bundle_manifest");
    let installed = root.join(".appsdk/contracts/sdk-bundle.manifest.json");
    assert_no_symlink_components(root, &installed, "sdk_installed_bundle_manifest");
    let authoring_value: Value = serde_json::from_slice(
        &fs::read(&authoring).unwrap_or_else(|_| fail("SDK_AUTHORING_BUNDLE_MIRROR_READ_FAILED")),
    )
    .unwrap_or_else(|_| fail("SDK_AUTHORING_BUNDLE_MIRROR_INVALID"));
    let installed_value: Value = serde_json::from_slice(
        &fs::read(&installed).unwrap_or_else(|_| fail("SDK_INSTALLED_BUNDLE_MIRROR_READ_FAILED")),
    )
    .unwrap_or_else(|_| fail("SDK_INSTALLED_BUNDLE_MIRROR_INVALID"));
    if authoring_value != installed_value {
        fail("SDK_AUTHORING_BUNDLE_MIRROR_DRIFT");
    }
    atomic_write_bytes(
        &authoring,
        SDK_BUNDLE_MANIFEST.as_bytes(),
        "SDK_AUTHORING_BUNDLE_MIRROR_WRITE_FAILED",
    );
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
    "owned_paths": ["playground/experiments/**", "protected/source/**", "tests/core/**"],
    "forbidden_paths": ["active/lib/**", "protected/**", "generated/**"],
    "verification_gates": ["fix_lifecycle_graph", "mainline_merge_identity"]
  }]
}
"#,
    );
    write_embedded_contract(
        root,
        "contracts/transitions/zone-transition-manifest.json",
        CANONICAL_ZONE_TRANSITION_CONTRACT,
    );
    write_embedded_contract(
        root,
        "contracts/transitions/zone-transition.manifest.json",
        CANONICAL_ZONE_TRANSITION_CONTRACT,
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
    let canonical_zone = project
        .pointer("/governance/zone_transition_contract")
        .and_then(Value::as_str)
        .map(|path| {
            matches!(
                path,
                "contracts/transitions/zone-transition.manifest.json"
                    | "contracts/transitions/zone-transition-manifest.json"
            )
        })
        .unwrap_or(false);
    let canonical_records = project
        .pointer("/governance/record_contracts")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .any(|value| value.as_str() == Some("contracts/records/record-graph.contract.json"))
        })
        .unwrap_or(false);
    let canonical_project = canonical_zone && canonical_records;
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
    let canonical_value: Value = serde_json::from_str(CANONICAL_ZONE_TRANSITION_CONTRACT)
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
        // Governance maps are project-owned projections. The SDK bundle
        // supplies schema/validation rules, but must not require byte-for-byte
        // equality with a generic SDK map; project modules may add or evolve
        // entries while retaining the same machine-readable contract.
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
            }
        }
    }
}

fn assert_lifecycle_producer_map_binding(root: &Path, project: &Value, module_id: &str) {
    for name in GOVERNANCE_MAP_NAMES {
        assert_no_symlink_components(
            root,
            &root.join(".appsdk/maps").join(name),
            "lifecycle_producer_map",
        );
    }
    assert_no_symlink_components(
        root,
        &root.join(".appsdk/maps/module-registry.json"),
        "lifecycle_producer_module_registry",
    );
    assert_governance_maps(root);
    // The producer's entries are compiled into the SDK. A project may extend
    // maps for other features, but it cannot rewrite or remove this contract.
    let mut maps = std::collections::HashMap::new();
    for name in GOVERNANCE_MAP_NAMES {
        let path = root.join(".appsdk/maps").join(name);
        assert_no_symlink_components(root, &path, "lifecycle_producer_map");
        let value: Value = serde_json::from_str(
            &fs::read_to_string(&path)
                .unwrap_or_else(|_| fail(format!("LIFECYCLE_PRODUCER_MAP_MISSING:{}", name))),
        )
        .unwrap_or_else(|_| fail(format!("LIFECYCLE_PRODUCER_MAP_INVALID:{}", name)));
        let key = match name {
            "resource-map.json" => "resources",
            "function-map.json" => "functions",
            "mainline-call-map.json" => "edges",
            "verification-map.json" => "gates",
            _ => unreachable!(),
        };
        if value.get("schema_version").and_then(Value::as_u64) != Some(1)
            || value
                .get(key)
                .and_then(Value::as_array)
                .is_none_or(|items| items.is_empty())
        {
            fail(format!("LIFECYCLE_PRODUCER_MAP_INVALID:{}", name));
        }
        maps.insert(name, value);
    }
    for (name, key) in [
        ("resource-map.json", "resources"),
        ("function-map.json", "functions"),
        ("mainline-call-map.json", "edges"),
        ("verification-map.json", "gates"),
    ] {
        let actual = maps
            .get(name)
            .unwrap()
            .get(key)
            .unwrap()
            .as_array()
            .unwrap();
        let canonical: Value = serde_json::from_str(canonical_governance_map(name))
            .unwrap_or_else(|_| fail(format!("LIFECYCLE_PRODUCER_MAP_INVALID:{}", name)));
        let required = canonical.get(key).unwrap().as_array().unwrap();
        let is_required =
            |entry: &Value| match name {
                "resource-map.json" => entry
                    .get("resource_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| {
                        matches!(
                            id,
                            "lifecycle_record_producer_input"
                                | "lifecycle_chain_producer_input"
                                | "fix_worktree"
                                | "fix_evidence_set"
                                | "fix_reproduction"
                        )
                    }),
                "function-map.json" => entry
                    .get("function_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| {
                        matches!(
                            id,
                            "lifecycle_record_producer" | "lifecycle_chain_record_producer"
                        )
                    }),
                "mainline-call-map.json" => entry
                    .get("chain_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| {
                        matches!(
                            id,
                            "lifecycle-record-production-v1"
                                | "lifecycle-record-chain-production-v1"
                        )
                    }),
                "verification-map.json" => {
                    entry
                        .get("gate_id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| {
                            matches!(
                                id,
                                "worktree_clean"
                                    | "baseline_reproduced"
                                    | "lifecycle_chain_record_producer"
                            )
                        })
                        || entry
                            .get("required_for")
                            .and_then(Value::as_array)
                            .is_some_and(|uses| {
                                uses.iter()
                                    .any(|use_case| use_case.as_str() == Some("promotion"))
                            })
                }
                _ => false,
            };
        let shares_required_identity = |candidate: &Value| {
            required.iter().any(|entry| match name {
                "resource-map.json" => candidate.get("resource_id") == entry.get("resource_id"),
                "function-map.json" => candidate.get("function_id") == entry.get("function_id"),
                "mainline-call-map.json" => candidate.get("chain_id") == entry.get("chain_id"),
                "verification-map.json" => candidate.get("gate_id") == entry.get("gate_id"),
                _ => false,
            })
        };
        for entry in required.iter().filter(|entry| is_required(entry)) {
            if actual
                .iter()
                .filter(|candidate| *candidate == entry)
                .count()
                != 1
            {
                fail(format!("LIFECYCLE_PRODUCER_MAP_TAMPERED:{}", name));
            }
        }
        // A project map may extend unrelated entries, but an entry with a
        // producer-owned identity cannot shadow the canonical declaration.
        if actual.iter().any(|candidate| {
            shares_required_identity(candidate) && !required.iter().any(|entry| candidate == entry)
        }) {
            fail(format!("LIFECYCLE_PRODUCER_MAP_TAMPERED:{}", name));
        }
    }

    let registry_path = root.join(".appsdk/maps/module-registry.json");
    assert_no_symlink_components(root, &registry_path, "lifecycle_producer_module_registry");
    let registry: Value = serde_json::from_str(
        &fs::read_to_string(&registry_path)
            .unwrap_or_else(|_| fail("LIFECYCLE_PRODUCER_MODULE_REGISTRY_MISSING")),
    )
    .unwrap_or_else(|_| fail("LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID"));
    if registry.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail("LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID");
    }
    let registry_modules = registry
        .get("modules")
        .and_then(Value::as_array)
        .filter(|modules| !modules.is_empty())
        .unwrap_or_else(|| fail("LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID"));
    let project_module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    let registered = registry_modules
        .iter()
        .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        .unwrap_or_else(|| fail("LIFECYCLE_PRODUCER_MODULE_BINDING_MISSING"));
    if registered.get("status").and_then(Value::as_str) != Some("active")
        || registered.get("owner").and_then(Value::as_str)
            != project_module.get("source_owner").and_then(Value::as_str)
    {
        fail("LIFECYCLE_PRODUCER_MODULE_BINDING_MISMATCH");
    }
    let registered_paths = registered
        .get("owned_paths")
        .and_then(Value::as_array)
        .filter(|paths| !paths.is_empty())
        .unwrap_or_else(|| fail("LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID"));
    for path in project_module
        .get("owned_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("LIFECYCLE_PRODUCER_MODULE_BINDING_MISMATCH"))
    {
        if !registered_paths.iter().any(|candidate| candidate == path) {
            fail("LIFECYCLE_PRODUCER_MODULE_BINDING_MISMATCH");
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

fn assert_sdk_lock(root: &Path, project: &Value) {
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
        if let Some(digest) = lock.get(key) {
            let digest = digest.as_str().unwrap_or("");
            if digest.len() != 71
                || !digest.starts_with("sha256:")
                || !digest[7..].chars().all(|c| c.is_ascii_hexdigit())
            {
                fail("INVALID_SDK_LOCK_DIGEST");
            }
        }
    }
    for key in [
        "bundle_digest",
        "bundle_manifest_digest",
        "previous_bundle_digest",
    ] {
        if let Some(digest) = lock.get(key) {
            let digest = digest.as_str().unwrap_or("");
            if digest.len() != 71
                || !digest.starts_with("sha256:")
                || !digest[7..].chars().all(|c| c.is_ascii_hexdigit())
            {
                fail("INVALID_SDK_BUNDLE_DIGEST");
            }
        }
    }
    if let Some(resources) = lock.get("bundle_resources") {
        if !resources.is_object() {
            fail("INVALID_SDK_BUNDLE_RESOURCES");
        }
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
    let project_target = root.join(candidate);
    let legacy_target = lib_root.join(candidate);
    assert_no_symlink_components(root, &project_target, "module_artifact");
    assert_no_symlink_components(root, &legacy_target, "module_artifact");
    // Current project contracts declare paths from the project root. Keep the
    // historical module-lib-relative form for existing generated contracts.
    let generated_root = required_str(
        project,
        "/governance/generated_root",
        "INVALID_GOVERNANCE_CONTRACT",
    )
    .trim_end_matches("/**")
    .trim_end_matches('/');
    let project_relative = relative.trim_end_matches('/');
    let project_declared = registry_path_matches(generated_root, project_relative)
        || project
            .get("modules")
            .and_then(Value::as_array)
            .and_then(|modules| {
                modules.iter().find(|module| {
                    module.get("module_id").and_then(Value::as_str) == Some(module_id)
                })
            })
            .and_then(|module| module.get("generated_outputs"))
            .and_then(Value::as_array)
            .is_some_and(|outputs| {
                outputs.iter().any(|output| {
                    output
                        .as_str()
                        .is_some_and(|pattern| registry_path_matches(pattern, project_relative))
                })
            });
    if project_declared {
        project_target
    } else {
        legacy_target
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
        let entry_type = entry
            .file_type()
            .unwrap_or_else(|_| fail(format!("HASH_TREE_READ_FAILED:{}", label)));
        let path = entry.path();
        // npm dependency trees are generated inputs; their .bin entries are
        // ordinary symlinks and must not contaminate source ownership hashes.
        let relative = path
            .strip_prefix(prefix)
            .unwrap_or_else(|_| fail(format!("HASH_TREE_PREFIX:{}", label)));
        if relative
            .components()
            .any(|component| component.as_os_str().to_str() == Some("node_modules"))
        {
            continue;
        }
        if entry_type.is_symlink() {
            fail(format!("HASH_TREE_SYMLINK:{}", label));
        }
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
        let dependency_frozen =
            dependency_module.get("stage").and_then(Value::as_str) == Some("frozen");
        if module.get("stage").and_then(Value::as_str) == Some("frozen") && !dependency_frozen {
            fail(format!(
                "MODULE_DEPENDENCY_NOT_FROZEN:{}:{}",
                module_id, dependency_id
            ));
        }
        // Dependency-first declaration is also the recursion bound for freshness
        // checks invoked directly by review admission, before project verification.
        let position = |id: &str| {
            modules
                .iter()
                .position(|entry| entry.get("module_id").and_then(Value::as_str) == Some(id))
        };
        if position(dependency_id) >= position(module_id) {
            fail(format!(
                "MODULE_DEPENDENCY_ORDER:{}:{}",
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
        module_artifact_matches_project(dependency_module, &artifact);
        let hash = record_str(&artifact, "/artifact_hash", "module-artifact");
        if !dependency_frozen {
            let current = build_module_artifact(root, project, dependency_module, dependency_id);
            if record_str(&current, "/artifact_hash", "dependency-artifact") != hash {
                fail(format!(
                    "MODULE_DEPENDENCY_ARTIFACT_STALE:{}:{}",
                    module_id, dependency_id
                ));
            }
        }
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

fn module_deployment_operations(module: &Value) -> Vec<&str> {
    let Some(value) = module.get("deployment_operations") else {
        // Existing service contracts retain both receipts until explicitly changed.
        return vec!["install", "restart"];
    };
    let values = value
        .as_array()
        .unwrap_or_else(|| fail("INVALID_DEPLOYMENT_OPERATIONS"));
    let mut operations = Vec::new();
    for value in values {
        let operation = value
            .as_str()
            .unwrap_or_else(|| fail("INVALID_DEPLOYMENT_OPERATIONS"));
        if !matches!(operation, "install" | "restart") || operations.contains(&operation) {
            fail("INVALID_DEPLOYMENT_OPERATIONS");
        }
        operations.push(operation);
    }
    operations
}

fn build_module_artifact(root: &Path, project: &Value, module: &Value, module_id: &str) -> Value {
    let source_hash = hash_module_paths(root, project, module, module_id, "owned_paths");
    let contract_hash = hash_module_paths(root, project, module, module_id, "contract_paths");
    let dependency_hashes = module_dependency_hashes(root, project, module, module_id);
    let build_command = module_build_command(module, module_id);
    let artifact_entries = hash_module_artifacts(root, project, module, module_id);
    let public_api_hash = module_public_api_hash(&artifact_entries);
    let mut unsigned = serde_json::Map::new();
    if let Some(operations) = module.get("deployment_operations") {
        let _ = module_deployment_operations(module);
        unsigned.insert("deployment_operations".into(), operations.clone());
    }
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
    if artifact.get("deployment_operations") != module.get("deployment_operations") {
        fail("MODULE_DEPLOYMENT_CONTRACT_DRIFT");
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
    assert_sdk_lock(root, project);
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
                    "appsdk promote --to source_implemented",
                    "appsdk promote --to contract_bound",
                    "rerun appsdk compile once the project is contract_bound"
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
    if multi_worktree && !multi_worker {
        fail("MERGE_QUEUE_COLLABORATION_REQUIRED");
    }
    multi_worktree
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
        let _ = module_deployment_operations(module);
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
        if let Some(version_base) = module.get("version_base").filter(|value| !value.is_null()) {
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
    // Publish the deterministic project projection before per-module builds.
    // A later module failure must not leave verification bound to an older
    // lifecycle snapshot; the projection itself contains no build output.
    write_artifact(root, &project);
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
                    | "previous_active_unavailable"
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
            | "previous_active_unavailable"
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

fn protected_archive_needs_version_restore(root: &Path, archive: &Path, artifact: &Value) -> bool {
    assert_no_symlink_components(root, archive, "protected_archive");
    let current = archive.join("module-artifact.json");
    let Ok(contents) = fs::read_to_string(current) else {
        return true;
    };
    let Ok(current_artifact) = serde_json::from_str::<Value>(&contents) else {
        return true;
    };
    current_artifact.get("artifact_hash") != artifact.get("artifact_hash")
}

fn restore_current_protected_archive_from_version(
    root: &Path,
    project: &Value,
    module: &Value,
    module_id: &str,
    version: &str,
    artifact: &Value,
    archive: &Path,
) {
    let protected_root = contract_root(root, project, "/governance/protected_root");
    let version_archive = protected_root
        .join("history-versions")
        .join(module_id)
        .join(version);
    if !version_archive.is_dir() {
        fail("PROTECTED_ARCHIVE_VERSION_HISTORY_MISSING");
    }
    assert_protected_archive_matches(root, module, artifact, &version_archive);
    let staging = archive.with_file_name(format!(
        ".{}.rehydrate-version.{}",
        module_id,
        std::process::id()
    ));
    let backup = archive.with_file_name(format!(
        ".{}.rehydrate-backup.{}",
        module_id,
        std::process::id()
    ));
    assert_no_symlink_components(root, &staging, "protected_archive_staging");
    assert_no_symlink_components(root, &backup, "protected_archive_backup");
    if staging.exists() || backup.exists() {
        fail("PROTECTED_ARCHIVE_RESTORE_STAGING_EXISTS");
    }
    copy_tree(&version_archive, &staging);
    if archive.exists() {
        fs::rename(archive, &backup).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_RESTORE_FAILED"));
    }
    if let Err(_) = fs::rename(&staging, archive) {
        if backup.exists() {
            let _ = fs::rename(&backup, archive);
        }
        fail("PROTECTED_ARCHIVE_RESTORE_FAILED");
    }
    if backup.exists() {
        fs::remove_dir_all(backup).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_RESTORE_FAILED"));
    }
}

fn restore_generated_module_from_archive(
    root: &Path,
    project: &Value,
    module_id: &str,
    artifact: &Value,
    archive: &Path,
) {
    let generated = module_generated_dir(root, project, module_id);
    let staging = generated_root(root, project)
        .join("rehydrate-generated")
        .join(format!("{}.{}", module_id, std::process::id()));
    assert_no_symlink_components(root, &generated, "generated_module");
    assert_no_symlink_components(root, &staging, "generated_module_staging");
    if generated.is_dir() {
        let existing = generated.join("module.compiled.json");
        if existing.is_file() {
            let current: Value = serde_json::from_str(
                &fs::read_to_string(existing)
                    .unwrap_or_else(|_| fail("MODULE_ARTIFACT_READ_FAILED")),
            )
            .unwrap_or_else(|_| fail("INVALID_MODULE_ARTIFACT"));
            if current == *artifact {
                return;
            }
        }
        fail("FROZEN_REHYDRATE_GENERATED_PROJECTION_MISMATCH");
    }
    if staging.exists() {
        fail("FROZEN_REHYDRATE_GENERATED_STAGING_EXISTS");
    }
    fs::create_dir_all(staging.join("lib"))
        .unwrap_or_else(|_| fail("FROZEN_REHYDRATE_GENERATED_FAILED"));
    atomic_write_json(
        &staging.join("module.compiled.json"),
        artifact,
        "FROZEN_REHYDRATE_GENERATED_FAILED",
    );
    for entry in artifact
        .get("artifacts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
    {
        let relative = record_str(entry, "/path", "module-artifact-entry");
        let source = safe_owned_path(&archive.join("library"), relative, "protected_library");
        let expected = record_str(entry, "/hash", "module-artifact-entry");
        if file_sha256(&source, "protected_library") != expected {
            fail("PROTECTED_ARCHIVE_LIBRARY_HASH_MISMATCH");
        }
        let target = staging.join("lib").join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|_| fail("FROZEN_REHYDRATE_GENERATED_FAILED"));
        }
        fs::copy(source, target).unwrap_or_else(|_| fail("FROZEN_REHYDRATE_GENERATED_FAILED"));
    }
    fs::create_dir_all(generated.parent().unwrap())
        .unwrap_or_else(|_| fail("FROZEN_REHYDRATE_GENERATED_FAILED"));
    fs::rename(staging, generated).unwrap_or_else(|_| fail("FROZEN_REHYDRATE_GENERATED_FAILED"));
}

fn verify_rehydrated_module(
    root: &Path,
    project: &Value,
    module: &Value,
    module_id: &str,
    version: &str,
    artifact: &Value,
    archive: &Path,
) {
    let generated = read_module_artifact(root, project, module_id);
    module_artifact_matches_project(module, &generated);
    if generated != *artifact {
        fail("FROZEN_REHYDRATE_GENERATED_PROJECTION_MISMATCH");
    }
    let project_artifact = read_compiled_artifact(root, project);
    assert_artifact_matches(project, &project_artifact);
    assert_protected_archive_matches(root, module, artifact, archive);
    assert_active_projection_matches(root, project, module_id, version, artifact);
}

fn rehydrate_frozen(root: &Path, module_id: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_project_contract(root, &project);
    assert_declared_contracts(root, &project, true);
    assert_goal_confirmed(root);
    assert_sdk_lock(root, &project);
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

    let version_archive = protected_root
        .join("history-versions")
        .join(module_id)
        .join(version);
    let from_version_archive = version_archive.is_dir();
    let artifact = if from_version_archive {
        let historical_artifact: Value = serde_json::from_str(
            &fs::read_to_string(version_archive.join("module-artifact.json"))
                .unwrap_or_else(|_| fail("MODULE_ARTIFACT_HISTORY_MISSING")),
        )
        .unwrap_or_else(|_| fail("INVALID_MODULE_ARTIFACT"));
        module_artifact_matches_project(module, &historical_artifact);
        assert_protected_archive_matches(root, module, &historical_artifact, &version_archive);
        historical_artifact
    } else {
        run_module_build(root, module, module_id);
        build_module_artifact(root, &project, module, module_id)
    };
    if from_version_archive {
        restore_generated_module_from_archive(
            root,
            &project,
            module_id,
            &artifact,
            &version_archive,
        );
    }
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
        assert_historical_frozen_record_graph(root, module_id, &artifact);
        assert_protected_archive_matches(root, module, &artifact, &archive);
        assert_active_projection_matches(root, &project, module_id, version, &artifact);
        // A complete current projection is already the idempotent result.
        // The previous Active archive is only needed when this invocation has
        // to restore it; older version metadata must not invalidate the
        // current Active/Protected projection.
        verify_rehydrated_module(
            root, &project, module, module_id, version, &artifact, &archive,
        );
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
        assert_historical_frozen_record_graph(root, module_id, &artifact);
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
            // Historical frozen records can name a predecessor whose
            // version archive was never published. The current target
            // archive is still independently immutable and sufficient for
            // this rehydrate; preserve the absence instead of fabricating a
            // predecessor or blocking a normal target restore.
            write_rehydrate_transaction(
                root,
                module_id,
                version,
                artifact_hash,
                "previous_active_unavailable",
            );
        } else if previous_active.is_dir() {
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
        assert_historical_frozen_record_graph(root, module_id, &artifact);
    }

    if archive.exists() {
        if protected_archive_needs_version_restore(root, &archive, &artifact) {
            restore_current_protected_archive_from_version(
                root, &project, module, module_id, version, &artifact, &archive,
            );
        } else {
            assert_protected_archive_matches(root, module, &artifact, &archive);
        }
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
        publish_active_rehydrated(root, module_id, version);
    }
    write_rehydrate_transaction(root, module_id, version, artifact_hash, "active_published");
    verify_rehydrated_module(
        root, &project, module, module_id, version, &artifact, &archive,
    );
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
    let history_root = protected_root.join("history").join(module_id);
    let freeze_name = freeze_record_name(module_id);
    let freeze = read_record(root, &freeze_name);
    let active_version = record_str(&freeze, "/active_version", &freeze_name);
    let archive = if history_root.exists() {
        protected_root
            .join("history-versions")
            .join(module_id)
            .join(active_version)
    } else {
        history_root
    };
    let staging_parent = archive
        .parent()
        .unwrap_or_else(|| fail("FREEZE_TRANSACTION_RECOVERY_FAILED"));
    let staging = staging_parent.join(format!(
        ".{}.staging.{}",
        active_version,
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

fn producer_input_path(root: &Path, raw: &str) -> PathBuf {
    if raw.is_empty() {
        fail("PRODUCER_INPUT_MISSING");
    }
    let path = Path::new(raw);
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    if fs::symlink_metadata(&full)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("PRODUCER_INPUT_SYMLINK");
    }
    if !full.is_file() {
        fail("PRODUCER_INPUT_NOT_FILE");
    }
    full
}

fn producer_string(record: &Value, path: &str, error: &str) -> String {
    record
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fail(error))
}

fn producer_issue(record: &Value, path: &str, error: &str) -> String {
    record
        .pointer(path)
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| fail(error))
}

fn producer_bool(record: &Value, path: &str, error: &str) {
    if record.pointer(path) != Some(&Value::Bool(true)) {
        fail(error);
    }
}

fn producer_command_dir(root: &Path, raw: &str) -> Result<PathBuf, &'static str> {
    let path = Path::new(raw);
    if raw.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("INVALID_BASELINE_COMMAND_DIRECTORY");
    }
    let full = root.join(path);
    assert_no_symlink_components(root, &full, "baseline_command");
    if !full.is_dir() {
        return Err("INVALID_BASELINE_COMMAND_DIRECTORY");
    }
    Ok(full)
}

fn producer_command(record: &Value) -> (String, Vec<String>, String, i32, String) {
    let command = record
        .get("command")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("BASELINE_COMMAND_MISSING"));
    let program = command
        .get("program")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .unwrap_or_else(|| fail("INVALID_BASELINE_COMMAND"))
        .to_string();
    let args = command
        .get("args")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_BASELINE_COMMAND"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| fail("INVALID_BASELINE_COMMAND"))
        })
        .collect::<Vec<_>>();
    let working_directory = command
        .get("working_directory")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail("INVALID_BASELINE_COMMAND"))
        .to_string();
    let expected_status = command
        .get("expected_exit_status")
        .and_then(Value::as_i64)
        .filter(|value| (-255..=255).contains(value) && *value != 0)
        .unwrap_or_else(|| fail("INVALID_BASELINE_COMMAND_STATUS"))
        as i32;
    let expected_error_token = command
        .get("expected_error_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail("BASELINE_ERROR_TOKEN_MISSING"))
        .to_string();
    (
        program,
        args,
        working_directory,
        expected_status,
        expected_error_token,
    )
}

fn producer_baseline_worktree(root: &Path, base_commit: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| fail("STAGING_NONCE_FAILED"))
        .as_nanos();
    let parent = root.join(".appsdk-control");
    let path = parent.join(format!(
        "producer-baseline-{}-{}",
        std::process::id(),
        nonce
    ));
    assert_no_symlink_components(root, &parent, "producer_baseline");
    fs::create_dir_all(&parent).unwrap_or_else(|_| fail("BASELINE_WORKTREE_CREATE_FAILED"));
    let output = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "worktree",
            "add",
            "--detach",
            path.to_str().unwrap_or("."),
            base_commit,
        ])
        .output()
        .unwrap_or_else(|_| fail("BASELINE_WORKTREE_CREATE_FAILED"));
    if !output.status.success() {
        fail("BASELINE_WORKTREE_CREATE_FAILED");
    }
    path
}

fn remove_producer_baseline_worktree(root: &Path, path: &Path) {
    let output = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "worktree",
            "remove",
            "--force",
            path.to_str().unwrap_or("."),
        ])
        .output()
        .unwrap_or_else(|_| fail("BASELINE_WORKTREE_CLEANUP_FAILED"));
    if !output.status.success() {
        fail("BASELINE_WORKTREE_CLEANUP_FAILED");
    }
}

fn producer_baseline_git_value(root: &Path, args: &[&str]) -> Result<String, &'static str> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|_| "PRODUCER_VCS_UNAVAILABLE")?;
    if !output.status.success() {
        return Err("PRODUCER_VCS_UNAVAILABLE");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(unix)]
fn producer_try_advisory_lock(file: &fs::File) -> Result<(), &'static str> {
    const LOCK_EX: c_int = 2;
    const LOCK_NB: c_int = 4;
    unsafe extern "C" {
        fn flock(fd: c_int, operation: c_int) -> c_int;
    }
    let result = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if result == 0 {
        return Ok(());
    }
    if std::io::Error::last_os_error().kind() == ErrorKind::WouldBlock {
        Err("PRODUCER_BUSY")
    } else {
        Err("PRODUCER_LOCK_FAILED")
    }
}

#[cfg(not(unix))]
fn producer_try_advisory_lock(_file: &fs::File) -> Result<(), &'static str> {
    Ok(())
}

fn producer_lock(root: &Path) -> fs::File {
    let control_dir = root.join(".appsdk-control");
    assert_no_symlink_components(root, &control_dir, "producer_lock");
    fs::create_dir_all(&control_dir).unwrap_or_else(|_| fail("PRODUCER_LOCK_FAILED"));
    let path = control_dir.join("lifecycle-record-producer.lock");
    assert_no_symlink_components(root, &path, "producer_lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
        .unwrap_or_else(|_| fail("PRODUCER_LOCK_FAILED"));
    producer_try_advisory_lock(&file).unwrap_or_else(|error| fail(error));
    file
}

fn producer_scope_hash(root: &Path, project: &Value, module: &Value, module_id: &str) -> String {
    let source_hash = hash_module_paths(root, project, module, module_id, "owned_paths");
    let contract_hash = hash_module_paths(root, project, module, module_id, "contract_paths");
    sha256(&canonical(&serde_json::json!({
        "module_id": module_id,
        "source_hash": source_hash,
        "contract_hash": contract_hash
    })))
}

fn producer_record_targets(
    root: &Path,
    module_id: &str,
    input: &Value,
    baseline_id: &str,
) -> Vec<(PathBuf, Value)> {
    let records = root.join(".appsdk").join("records");
    let evidence_dir = records.join("evidence").join(module_id);
    let worktree = input
        .get("worktree")
        .cloned()
        .unwrap_or_else(|| fail("PRODUCER_WORKTREE_MISSING"));
    let reproduction = input
        .get("reproduction")
        .cloned()
        .unwrap_or_else(|| fail("PRODUCER_REPRODUCTION_MISSING"));
    let baseline = input
        .get("baseline_evidence")
        .cloned()
        .unwrap_or_else(|| fail("PRODUCER_BASELINE_EVIDENCE_MISSING"));
    vec![
        (
            records.join(module_record_name("worktree-record", module_id)),
            worktree,
        ),
        (
            records.join(module_record_name("reproduction-record", module_id)),
            reproduction,
        ),
        (evidence_dir.join(format!("{}.json", baseline_id)), baseline),
    ]
}

fn producer_stable_id(prefix: &str, value: &Value) -> String {
    let digest = sha256(&canonical(value));
    format!(
        "{}-{}",
        prefix,
        digest.strip_prefix("sha256:").unwrap_or(&digest)
    )
}

fn lifecycle_chain_review_identity(
    promotion_id: &str,
    fix_candidate_id: &str,
    reviewer: &Value,
    verdict: &str,
    evidence_ids: &Value,
    project_bindings: Option<&Value>,
) -> Value {
    let mut identity = serde_json::json!({
        "promotion_id": promotion_id,
        "fix_candidate_id": fix_candidate_id,
        "reviewer": reviewer,
        "verdict": verdict,
        "evidence_ids": evidence_ids
    });
    if let Some(bindings) = project_bindings {
        identity["project_bindings"] = bindings.clone();
    }
    identity
}

fn assert_lifecycle_chain_review_identity(review: &Value) {
    let project_bindings = review.get("project_bindings");
    if project_bindings.is_some_and(|value| !value.is_object()) {
        fail("INVALID_REVIEW_PROJECT_BINDINGS");
    }
    let reviewer = review
        .get("reviewer")
        .filter(|value| {
            value
                .get("adapter")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
                && value
                    .get("identity")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty())
        })
        .unwrap_or_else(|| fail("INVALID_REVIEW_RECORD"));
    let identity = lifecycle_chain_review_identity(
        record_str(review, "/promotion_id", "review-record.json"),
        record_str(review, "/fix_candidate_id", "review-record.json"),
        reviewer,
        record_str(review, "/verdict", "review-record.json"),
        &Value::Array(record_array(review, "/evidence_ids", "review-record.json").clone()),
        project_bindings,
    );
    if record_str(review, "/review_id", "review-record.json")
        != producer_stable_id("review", &identity)
    {
        fail("ARCHITECTURE_REVIEW_IDENTITY_MISMATCH");
    }
}

fn assert_lifecycle_chain_review_identity_or_frozen_legacy(
    root: &Path,
    module_id: &str,
    review: &Value,
) {
    let review_id = record_str(review, "/review_id", "review-record.json");
    let identity_matches = review.get("project_bindings").is_none_or(Value::is_object)
        && review
            .get("reviewer")
            .and_then(Value::as_object)
            .and_then(|reviewer| {
                Some((
                    reviewer.get("adapter")?.as_str()?,
                    reviewer.get("identity")?.as_str()?,
                ))
            })
            .is_some_and(|(adapter, identity)| !adapter.is_empty() && !identity.is_empty())
        && review.get("promotion_id").and_then(Value::as_str).is_some()
        && review
            .get("fix_candidate_id")
            .and_then(Value::as_str)
            .is_some()
        && review.get("verdict").and_then(Value::as_str).is_some()
        && review
            .get("evidence_ids")
            .and_then(Value::as_array)
            .is_some()
        && producer_stable_id(
            "review",
            &lifecycle_chain_review_identity(
                review["promotion_id"].as_str().unwrap(),
                review["fix_candidate_id"].as_str().unwrap(),
                review.get("reviewer").unwrap(),
                review["verdict"].as_str().unwrap(),
                &review["evidence_ids"],
                review.get("project_bindings"),
            ),
        ) == review_id;
    if identity_matches {
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
    if !matches!(
        module.get("stage").and_then(Value::as_str),
        Some("frozen" | "retired")
    ) {
        fail("ARCHITECTURE_REVIEW_IDENTITY_MISMATCH");
    }

    let freeze_name = module_record_name("freeze-record", module_id);
    let freeze = read_record(root, &freeze_name);
    let promotion_name = module_record_name("promotion-record", module_id);
    let promotion = read_record(root, &promotion_name);
    if record_str(&freeze, "/module_id", &freeze_name) != module_id
        || record_str(&freeze, "/review_id", &freeze_name) != review_id
        || record_str(&freeze, "/promotion_id", &freeze_name)
            != record_str(&promotion, "/promotion_id", &promotion_name)
        || record_str(&promotion, "/review_id", &promotion_name) != review_id
        || review.get("verdict").and_then(Value::as_str) != Some("pass")
    {
        fail("ARCHITECTURE_REVIEW_IDENTITY_MISMATCH");
    }
}

fn producer_transaction_dir(root: &Path, module_id: &str) -> PathBuf {
    root.join(".appsdk")
        .join("transactions")
        .join(format!("producer-{}", module_id))
}

fn producer_record_transaction_validate_marker(root: &Path, module_id: &str, input_hash: &str) {
    let transaction = producer_transaction_dir(root, module_id);
    let marker_path = transaction.join("marker.json");
    if !transaction.exists() || !marker_path.is_file() {
        return;
    }
    assert_no_symlink_components(root, &transaction, "lifecycle_producer_transaction");
    let marker: Value = serde_json::from_str(
        &fs::read_to_string(&marker_path)
            .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_MARKER_INVALID")),
    )
    .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_MARKER_INVALID"));
    if marker.get("schema_version").and_then(Value::as_u64) != Some(1)
        || marker.get("module_id").and_then(Value::as_str) != Some(module_id)
        || marker.get("input_hash").and_then(Value::as_str) != Some(input_hash)
        || marker.get("phase").and_then(Value::as_str) != Some("commit")
    {
        fail("PRODUCER_TRANSACTION_MARKER_MISMATCH");
    }
    let entries = marker
        .get("records")
        .and_then(Value::as_array)
        .filter(|entries| entries.len() == 3)
        .unwrap_or_else(|| fail("PRODUCER_TRANSACTION_MARKER_INVALID"));
    let records_root = root.join(".appsdk").join("records");
    let expected_fixed_targets = [
        records_root.join(module_record_name("worktree-record", module_id)),
        records_root.join(module_record_name("reproduction-record", module_id)),
    ];
    let expected_evidence_root = records_root.join("evidence").join(module_id);
    for (index, entry) in entries.iter().enumerate() {
        let relative = entry
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("PRODUCER_TRANSACTION_MARKER_INVALID"));
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            || !relative.starts_with(".appsdk/records/")
        {
            fail("PRODUCER_TRANSACTION_TARGET_INVALID");
        }
        let target = root.join(relative_path);
        if index < expected_fixed_targets.len() {
            if target != expected_fixed_targets[index] {
                fail("PRODUCER_TRANSACTION_TARGET_INVALID");
            }
        } else if target.parent() != Some(expected_evidence_root.as_path())
            || target
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| {
                    let Some(digest) = name
                        .strip_prefix("baseline-")
                        .and_then(|name| name.strip_suffix(".json"))
                    else {
                        return true;
                    };
                    digest.len() != 64
                        || !digest
                            .chars()
                            .all(|character| character.is_ascii_hexdigit())
                })
        {
            fail("PRODUCER_TRANSACTION_TARGET_INVALID");
        }
        let staging_name = entry
            .get("staging")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty() && !name.contains('/') && !name.contains('\\'))
            .unwrap_or_else(|| fail("PRODUCER_TRANSACTION_MARKER_INVALID"));
        if staging_name != format!("record-{}.json", index) {
            fail("PRODUCER_TRANSACTION_MARKER_INVALID");
        }
    }
}

fn producer_record_transaction_recover<F>(
    root: &Path,
    module_id: &str,
    input_hash: &str,
    validate: F,
) -> Option<Vec<(PathBuf, Value)>>
where
    F: FnOnce(&[(PathBuf, Value)]),
{
    let transaction = producer_transaction_dir(root, module_id);
    if !transaction.exists() {
        return None;
    }
    assert_no_symlink_components(root, &transaction, "lifecycle_producer_transaction");
    let marker_path = transaction.join("marker.json");
    if !marker_path.is_file() {
        // No commit marker means staging never became durable. It is safe to
        // discard that incomplete transaction and run the producer again.
        fs::remove_dir_all(&transaction)
            .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_CLEANUP_FAILED"));
        return None;
    }
    let marker: Value = serde_json::from_str(
        &fs::read_to_string(&marker_path)
            .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_MARKER_INVALID")),
    )
    .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_MARKER_INVALID"));
    if marker.get("schema_version").and_then(Value::as_u64) != Some(1)
        || marker.get("module_id").and_then(Value::as_str) != Some(module_id)
        || marker.get("input_hash").and_then(Value::as_str) != Some(input_hash)
        || marker.get("phase").and_then(Value::as_str) != Some("commit")
    {
        fail("PRODUCER_TRANSACTION_MARKER_MISMATCH");
    }
    let entries = marker
        .get("records")
        .and_then(Value::as_array)
        .filter(|entries| entries.len() == 3)
        .unwrap_or_else(|| fail("PRODUCER_TRANSACTION_MARKER_INVALID"));
    let records_root = root.join(".appsdk").join("records");
    let expected_fixed_targets = [
        records_root.join(module_record_name("worktree-record", module_id)),
        records_root.join(module_record_name("reproduction-record", module_id)),
    ];
    let expected_evidence_root = records_root.join("evidence").join(module_id);
    let mut recovered = Vec::new();
    let mut pending_links = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let relative = entry
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("PRODUCER_TRANSACTION_MARKER_INVALID"));
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            || !relative.starts_with(".appsdk/records/")
        {
            fail("PRODUCER_TRANSACTION_TARGET_INVALID");
        }
        let target = root.join(relative_path);
        if !target.starts_with(&records_root) {
            fail("PRODUCER_TRANSACTION_TARGET_INVALID");
        }
        if index < expected_fixed_targets.len() {
            if target != expected_fixed_targets[index] {
                fail("PRODUCER_TRANSACTION_TARGET_INVALID");
            }
        } else if target.parent() != Some(expected_evidence_root.as_path())
            || target
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| {
                    let Some(digest) = name
                        .strip_prefix("baseline-")
                        .and_then(|name| name.strip_suffix(".json"))
                    else {
                        return true;
                    };
                    digest.len() != 64
                        || !digest
                            .chars()
                            .all(|character| character.is_ascii_hexdigit())
                })
        {
            fail("PRODUCER_TRANSACTION_TARGET_INVALID");
        }
        assert_no_symlink_components(root, &target, "lifecycle_producer_record");
        let staging_name = entry
            .get("staging")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty() && !name.contains('/') && !name.contains('\\'))
            .unwrap_or_else(|| fail("PRODUCER_TRANSACTION_MARKER_INVALID"));
        if staging_name != format!("record-{}.json", index) {
            fail("PRODUCER_TRANSACTION_MARKER_INVALID");
        }
        let expected_hash = entry
            .get("digest")
            .and_then(Value::as_str)
            .filter(|digest| digest.starts_with("sha256:"))
            .unwrap_or_else(|| fail("PRODUCER_TRANSACTION_MARKER_INVALID"));
        let staging = transaction.join(staging_name);
        assert_no_symlink_components(root, &staging, "lifecycle_producer_staging");
        let bytes = if target.exists() {
            if file_sha256(&target, "lifecycle_producer_record") != expected_hash {
                fail("PRODUCER_TRANSACTION_TARGET_CONFLICT");
            }
            fs::read(&target).unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_RECOVERY_FAILED"))
        } else {
            if !staging.is_file()
                || file_sha256(&staging, "lifecycle_producer_staging") != expected_hash
            {
                fail("PRODUCER_TRANSACTION_STAGING_MISSING");
            }
            pending_links.push((staging.clone(), target.clone()));
            fs::read(&staging).unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_RECOVERY_FAILED"))
        };
        let record: Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_RECORD_INVALID"));
        if index == 2 {
            let expected_name = format!(
                "{}.json",
                producer_string(
                    &record,
                    "/evidence_id",
                    "PRODUCER_TRANSACTION_RECORD_INVALID"
                )
            );
            if target.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
                fail("PRODUCER_TRANSACTION_TARGET_INVALID");
            }
        }
        recovered.push((target, record));
    }
    // Validate the complete staged graph against the current candidate before
    // publishing any missing records. A mismatch must leave the transaction
    // available for diagnosis and must not publish a partial graph.
    validate(&recovered);
    for (staging, target) in pending_links {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_RECOVERY_FAILED"));
        }
        fs::hard_link(&staging, &target)
            .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_RECOVERY_FAILED"));
    }
    fs::remove_dir_all(&transaction)
        .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_CLEANUP_FAILED"));
    Some(recovered)
}

fn producer_durable_json(target: &Path, value: &Value, error: &str) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| fail("STAGING_NONCE_FAILED"))
        .as_nanos();
    let staging = target.with_extension(format!("staging.{}.{}", std::process::id(), nonce));
    let bytes = (serde_json::to_string_pretty(value).unwrap() + "\n").into_bytes();
    let result = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staging)
        .and_then(|mut file| {
            file.write_all(&bytes)?;
            file.sync_all()
        });
    if result.is_err() {
        let _ = fs::remove_file(&staging);
        fail(error);
    }
    fs::rename(&staging, target).unwrap_or_else(|_| fail(error));
    if let Some(parent) = target.parent() {
        if let Ok(file) = OpenOptions::new().read(true).open(parent) {
            let _ = file.sync_all();
        }
    }
}

fn producer_commit_records(
    root: &Path,
    module_id: &str,
    input_hash: &str,
    targets: &[(PathBuf, Value)],
) {
    let transaction = producer_transaction_dir(root, module_id);
    assert_no_symlink_components(root, &transaction, "lifecycle_producer_transaction");
    fs::create_dir_all(&transaction).unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_WRITE_FAILED"));
    assert_no_symlink_components(root, &transaction, "lifecycle_producer_transaction");
    let mut entries = Vec::new();
    for (index, (target, record)) in targets.iter().enumerate() {
        assert_no_symlink_components(root, target, "lifecycle_producer_record");
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_WRITE_FAILED"));
        }
        let staging_name = format!("record-{}.json", index);
        let staging = transaction.join(&staging_name);
        let bytes = (serde_json::to_string_pretty(record).unwrap() + "\n").into_bytes();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
            .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_WRITE_FAILED"));
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_WRITE_FAILED"));
        entries.push(serde_json::json!({
            "target": target.strip_prefix(root).unwrap_or(target).to_string_lossy(),
            "staging": staging_name,
            "digest": digest_bytes(&bytes)
        }));
    }
    producer_durable_json(
        &transaction.join("marker.json"),
        &serde_json::json!({
            "schema_version": 1,
            "module_id": module_id,
            "input_hash": input_hash,
            "phase": "commit",
            "records": entries
        }),
        "PRODUCER_TRANSACTION_WRITE_FAILED",
    );
    for (index, (target, record)) in targets.iter().enumerate() {
        let staging = transaction.join(format!("record-{}.json", index));
        if target.exists() {
            if file_sha256(target, "lifecycle_producer_record")
                != digest_bytes(
                    &(serde_json::to_string_pretty(record).unwrap() + "\n").into_bytes(),
                )
            {
                fail("PRODUCER_TRANSACTION_TARGET_CONFLICT");
            }
        } else {
            fs::hard_link(&staging, target)
                .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_WRITE_FAILED"));
        }
        let _ = fs::remove_file(staging);
    }
    fs::remove_dir_all(&transaction)
        .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_CLEANUP_FAILED"));
}

fn assert_produced_record_shapes(targets: &[(PathBuf, Value)], module_id: &str) {
    if targets.len() != 3 {
        fail("PRODUCER_RECORD_SET_INVALID");
    }
    let worktree = &targets[0].1;
    for path in [
        "/worktree_id",
        "/issue_id",
        "/module_id",
        "/base_ref",
        "/base_commit",
        "/branch",
        "/head_commit",
        "/isolation_mode",
        "/scope_hash",
        "/created_at",
    ] {
        producer_string(worktree, path, "PRODUCER_RECORD_SCHEMA_INVALID");
    }
    producer_bool(worktree, "/initial_clean", "PRODUCER_RECORD_SCHEMA_INVALID");
    producer_bool(worktree, "/final_clean", "PRODUCER_RECORD_SCHEMA_INVALID");
    if producer_string(
        worktree,
        "/isolation_mode",
        "PRODUCER_RECORD_SCHEMA_INVALID",
    ) != "isolated_worktree"
    {
        fail("PRODUCER_RECORD_SCHEMA_INVALID");
    }
    if producer_string(worktree, "/module_id", "PRODUCER_RECORD_SCHEMA_INVALID") != module_id {
        fail("PRODUCER_RECORD_SCHEMA_INVALID");
    }
    let issue_id = producer_issue(worktree, "/issue_id", "PRODUCER_RECORD_SCHEMA_INVALID");
    assert_bug_tracker_triage_evidence(worktree, &issue_id, None, true);

    let reproduction = &targets[1].1;
    for path in [
        "/reproduction_id",
        "/issue_id",
        "/module_id",
        "/worktree_id",
        "/base_commit",
        "/baseline_evidence_id",
        "/first_divergence",
        "/created_at",
    ] {
        producer_string(reproduction, path, "PRODUCER_RECORD_SCHEMA_INVALID");
    }
    if reproduction
        .get("input_hashes")
        .and_then(Value::as_array)
        .is_none_or(|values| {
            values.is_empty() || values.iter().any(|value| value.as_str().is_none())
        })
        || reproduction.get("result").and_then(Value::as_str) != Some("reproduced")
        || producer_string(reproduction, "/module_id", "PRODUCER_RECORD_SCHEMA_INVALID")
            != module_id
        || producer_string(reproduction, "/issue_id", "PRODUCER_RECORD_SCHEMA_INVALID") != issue_id
        || producer_string(
            reproduction,
            "/worktree_id",
            "PRODUCER_RECORD_SCHEMA_INVALID",
        ) != producer_string(worktree, "/worktree_id", "PRODUCER_RECORD_SCHEMA_INVALID")
    {
        fail("PRODUCER_RECORD_SCHEMA_INVALID");
    }

    let evidence = &targets[2].1;
    for path in [
        "/evidence_id",
        "/issue_id",
        "/experiment_id",
        "/phase",
        "/kind",
        "/source_commit",
        "/scope/module_id",
        "/producer/adapter",
        "/producer/identity",
        "/created_at",
        "/expires_at",
        "/scope_hash",
    ] {
        producer_string(evidence, path, "PRODUCER_RECORD_SCHEMA_INVALID");
    }
    if !matches!(
        evidence.get("phase").and_then(Value::as_str),
        Some("baseline_reproduction")
    ) || !matches!(
        evidence.get("kind").and_then(Value::as_str),
        Some("red_test" | "sample_replay" | "gate" | "runtime")
    ) || !matches!(
        evidence.get("result").and_then(Value::as_str),
        Some("pass" | "fail")
    ) || evidence
        .get("input_hashes")
        .and_then(Value::as_array)
        .is_none_or(|values| {
            values.is_empty() || values.iter().any(|value| value.as_str().is_none())
        })
        || producer_string(
            evidence,
            "/scope/module_id",
            "PRODUCER_RECORD_SCHEMA_INVALID",
        ) != module_id
        || producer_string(evidence, "/issue_id", "PRODUCER_RECORD_SCHEMA_INVALID") != issue_id
        || producer_string(evidence, "/source_commit", "PRODUCER_RECORD_SCHEMA_INVALID")
            != producer_string(worktree, "/base_commit", "PRODUCER_RECORD_SCHEMA_INVALID")
        || producer_string(evidence, "/evidence_id", "PRODUCER_RECORD_SCHEMA_INVALID")
            != producer_string(
                reproduction,
                "/baseline_evidence_id",
                "PRODUCER_RECORD_SCHEMA_INVALID",
            )
    {
        fail("PRODUCER_RECORD_SCHEMA_INVALID");
    }
}

fn assert_recovered_record_bindings(
    targets: &[(PathBuf, Value)],
    module_id: &str,
    issue_id: &str,
    base_ref: &str,
    base_commit: &str,
    head_commit: &str,
    branch: &str,
    scope_hash: &str,
    worktree_id: &str,
    reproduction_id: &str,
    baseline_id: &str,
    input_hashes: &[String],
    command: &Value,
    actual_status: i32,
    output_hash: &str,
    bug_triage: Option<&Value>,
) {
    let worktree = &targets[0].1;
    if producer_issue(
        worktree,
        "/issue_id",
        "PRODUCER_RECOVERY_WORKTREE_BINDING_MISMATCH",
    ) != issue_id
        || producer_string(
            worktree,
            "/base_ref",
            "PRODUCER_RECOVERY_WORKTREE_BINDING_MISMATCH",
        ) != base_ref
        || producer_string(
            worktree,
            "/base_commit",
            "PRODUCER_RECOVERY_WORKTREE_BINDING_MISMATCH",
        ) != base_commit
        || producer_string(
            worktree,
            "/head_commit",
            "PRODUCER_RECOVERY_WORKTREE_BINDING_MISMATCH",
        ) != head_commit
        || producer_string(
            worktree,
            "/branch",
            "PRODUCER_RECOVERY_WORKTREE_BINDING_MISMATCH",
        ) != branch
        || producer_string(
            worktree,
            "/scope_hash",
            "PRODUCER_RECOVERY_WORKTREE_BINDING_MISMATCH",
        ) != scope_hash
        || producer_string(
            worktree,
            "/worktree_id",
            "PRODUCER_RECOVERY_WORKTREE_BINDING_MISMATCH",
        ) != worktree_id
        || worktree.get("initial_clean") != Some(&Value::Bool(true))
        || worktree.get("final_clean") != Some(&Value::Bool(true))
        || bug_triage.is_some_and(|expected| worktree.get("bug_triage") != Some(expected))
        || bug_triage.is_none() && worktree.get("bug_triage").is_some()
    {
        fail("PRODUCER_RECOVERY_WORKTREE_BINDING_MISMATCH");
    }
    assert_produced_record_shapes(targets, module_id);

    let reproduction = &targets[1].1;
    if producer_string(
        reproduction,
        "/issue_id",
        "PRODUCER_RECOVERY_RECORD_BINDING_MISMATCH",
    ) != issue_id
        || producer_string(
            reproduction,
            "/module_id",
            "PRODUCER_RECOVERY_RECORD_BINDING_MISMATCH",
        ) != module_id
        || producer_string(
            reproduction,
            "/worktree_id",
            "PRODUCER_RECOVERY_RECORD_BINDING_MISMATCH",
        ) != worktree_id
        || producer_string(
            reproduction,
            "/base_commit",
            "PRODUCER_RECOVERY_RECORD_BINDING_MISMATCH",
        ) != base_commit
        || producer_string(
            reproduction,
            "/reproduction_id",
            "PRODUCER_RECOVERY_RECORD_BINDING_MISMATCH",
        ) != reproduction_id
        || reproduction.get("input_hashes") != Some(&serde_json::json!(input_hashes))
        || producer_string(
            reproduction,
            "/baseline_evidence_id",
            "PRODUCER_RECOVERY_RECORD_BINDING_MISMATCH",
        ) != baseline_id
    {
        fail("PRODUCER_RECOVERY_RECORD_BINDING_MISMATCH");
    }

    let evidence = &targets[2].1;
    if producer_string(
        evidence,
        "/issue_id",
        "PRODUCER_RECOVERY_BASELINE_BINDING_MISMATCH",
    ) != issue_id
        || producer_string(
            evidence,
            "/scope/module_id",
            "PRODUCER_RECOVERY_BASELINE_BINDING_MISMATCH",
        ) != module_id
        || producer_string(
            evidence,
            "/scope_hash",
            "PRODUCER_RECOVERY_BASELINE_BINDING_MISMATCH",
        ) != scope_hash
        || producer_string(
            evidence,
            "/source_commit",
            "PRODUCER_RECOVERY_BASELINE_BINDING_MISMATCH",
        ) != base_commit
        || producer_string(
            evidence,
            "/evidence_id",
            "PRODUCER_RECOVERY_BASELINE_BINDING_MISMATCH",
        ) != baseline_id
        || evidence.get("input_hashes") != Some(&serde_json::json!(input_hashes))
        || evidence.get("command") != Some(command)
        || evidence.get("exit_status").and_then(Value::as_i64) != Some(i64::from(actual_status))
        || producer_string(
            evidence,
            "/output_hash",
            "PRODUCER_RECOVERY_BASELINE_BINDING_MISMATCH",
        ) != output_hash
        || evidence.get("producer")
            != Some(&serde_json::json!({
                "adapter": "appsdk",
                "identity": "appsdk-lifecycle-record-producer"
            }))
    {
        fail("PRODUCER_RECOVERY_BASELINE_BINDING_MISMATCH");
    }
}

fn produce_lifecycle_records(root: &Path, module_id: &str, input_path: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_declared_contracts(root, &project, true);
    assert_lifecycle_producer_map_binding(root, &project, module_id);
    assert_goal_confirmed(root);
    let goal = read_goal(root);
    let input_file = producer_input_path(root, input_path);
    let input: Value = serde_json::from_str(
        &fs::read_to_string(input_file).unwrap_or_else(|_| fail("PRODUCER_INPUT_READ_FAILED")),
    )
    .unwrap_or_else(|_| fail("INVALID_PRODUCER_INPUT"));
    if !input.is_object() {
        fail("INVALID_PRODUCER_INPUT");
    }
    // Validate again after taking the lock so a concurrent map edit cannot be
    // accepted between the read-only preflight and the record transaction.
    let _producer_lock = producer_lock(root);
    assert_declared_contracts(root, &project, true);
    assert_lifecycle_producer_map_binding(root, &project, module_id);
    if producer_string(&input, "/goal_id", "PRODUCER_GOAL_MISSING")
        != producer_string(&goal, "/goal_id", "INVALID_GOAL_CLARIFICATION_RECORD")
    {
        fail("PRODUCER_GOAL_MISMATCH");
    }
    let module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    let stage = producer_string(module, "/stage", "INVALID_MODULE_CONTRACT");
    if stage == "frozen" || stage == "retired" {
        fail(format!("PRODUCER_MODULE_STAGE_FORBIDDEN:{}", stage));
    }
    let input_hash = sha256(&canonical(&input));
    // A durable marker is structurally checked during preflight so malformed
    // transactions fail with their own diagnostic; record recovery itself is
    // still delayed until all current-candidate gates have passed below.
    producer_record_transaction_validate_marker(root, module_id, &input_hash);
    let records_root = root.join(".appsdk").join("records");
    if !producer_transaction_dir(root, module_id).exists() {
        for target in [
            records_root.join(module_record_name("worktree-record", module_id)),
            records_root.join(module_record_name("reproduction-record", module_id)),
        ] {
            assert_no_symlink_components(root, &target, "record_control");
            if target.exists() {
                fail(format!(
                    "LIFECYCLE_RECORD_EXISTS:{}",
                    target.strip_prefix(root).unwrap_or(&target).display()
                ));
            }
        }
    }
    let worktree = input
        .get("worktree")
        .unwrap_or_else(|| fail("PRODUCER_WORKTREE_MISSING"));
    let reproduction = input
        .get("reproduction")
        .unwrap_or_else(|| fail("PRODUCER_REPRODUCTION_MISSING"));
    let baseline = input
        .get("baseline_evidence")
        .unwrap_or_else(|| fail("PRODUCER_BASELINE_EVIDENCE_MISSING"));
    // These paths are module-stable, so reject a repeated producer call before
    // any later clean-worktree gate can mask the idempotent result.
    let expected_scope_hash = producer_scope_hash(root, &project, module, module_id);
    for path in [
        "/worktree_id",
        "/module_id",
        "/base_ref",
        "/base_commit",
        "/branch",
        "/head_commit",
        "/scope_hash",
    ] {
        producer_string(worktree, path, "INVALID_WORKTREE_RECORD");
    }
    producer_bool(worktree, "/initial_clean", "WORKTREE_INITIAL_NOT_CLEAN");
    producer_bool(worktree, "/final_clean", "WORKTREE_FINAL_NOT_CLEAN");
    if producer_string(worktree, "/module_id", "INVALID_WORKTREE_RECORD") != module_id
        || producer_string(worktree, "/isolation_mode", "INVALID_WORKTREE_RECORD")
            != "isolated_worktree"
    {
        fail("PRODUCER_MODULE_MISMATCH");
    }
    if producer_string(worktree, "/scope_hash", "INVALID_WORKTREE_RECORD") != expected_scope_hash {
        fail(format!(
            "PRODUCER_SCOPE_MISMATCH:expected={}",
            expected_scope_hash
        ));
    }
    let worktree_issue = producer_issue(worktree, "/issue_id", "INVALID_WORKTREE_RECORD");
    if !worktree_issue.is_empty()
        && worktree_issue != "none"
        && !worktree_issue.starts_with("legacy-")
        && worktree.get("bug_triage").is_none()
    {
        fail("BUG_TRIAGE_MISSING");
    }
    assert_bug_tracker_triage_evidence(worktree, &worktree_issue, Some(root), true);
    if goal
        .get("issue_id")
        .and_then(Value::as_str)
        .is_some_and(|issue| issue != worktree_issue)
    {
        fail("PRODUCER_GOAL_ISSUE_MISMATCH");
    }
    if producer_string(worktree, "/base_commit", "INVALID_WORKTREE_RECORD")
        == producer_string(worktree, "/head_commit", "INVALID_WORKTREE_RECORD")
    {
        // A baseline-only candidate is valid for SDK smoke tests and for an
        // adapter that has not committed source changes yet; the later
        // FixCandidate gate still binds the actual candidate commit.
    }
    let current_branch = git_value(
        root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        "PRODUCER_BRANCH_UNAVAILABLE",
    );
    if matches!(current_branch.as_str(), "main" | "master" | "v4-cordis") {
        fail("PRODUCER_PROTECTED_BRANCH");
    }
    if current_branch != producer_string(worktree, "/branch", "INVALID_WORKTREE_RECORD") {
        fail("PRODUCER_BRANCH_MISMATCH");
    }
    let worktree_list = git_value(
        root,
        &["worktree", "list", "--porcelain"],
        "PRODUCER_WORKTREE_UNAVAILABLE",
    );
    let current_root = root
        .canonicalize()
        .unwrap_or_else(|_| fail("PRODUCER_WORKTREE_UNAVAILABLE"));
    if !worktree_list.lines().any(|line| {
        line.strip_prefix("worktree ")
            .and_then(|path| Path::new(path).canonicalize().ok())
            .is_some_and(|path| path == current_root)
    }) {
        fail("PRODUCER_WORKTREE_NOT_REGISTERED");
    }
    let status = git_value(
        root,
        &["status", "--porcelain", "--untracked-files=all"],
        "PRODUCER_VCS_UNAVAILABLE",
    );
    if !status.is_empty() {
        let transaction = producer_transaction_dir(root, module_id);
        let marker_path = transaction.join("marker.json");
        let transaction_targets = fs::read_to_string(marker_path)
            .ok()
            .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
            .and_then(|marker| marker.get("records").cloned())
            .and_then(|records| records.as_array().cloned())
            .map(|records| {
                records
                    .iter()
                    .filter_map(|entry| entry.get("target").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            });
        let transaction_only = status.lines().all(|line| {
            line.get(3..).map(str::trim).is_some_and(|path| {
                transaction_targets
                    .as_ref()
                    .is_some_and(|targets| targets.iter().any(|target| target == path))
                    || path.starts_with(&format!(".appsdk/transactions/producer-{}/", module_id))
            })
        });
        if !transaction_only {
            fail("PRODUCER_WORKTREE_DIRTY");
        }
    }
    let base_commit = producer_string(worktree, "/base_commit", "INVALID_WORKTREE_RECORD");
    let head_commit = producer_string(worktree, "/head_commit", "INVALID_WORKTREE_RECORD");
    if git_value(
        root,
        &["rev-parse", &base_commit],
        "PRODUCER_BASE_COMMIT_INVALID",
    ) != base_commit
        || git_value(
            root,
            &["rev-parse", &head_commit],
            "PRODUCER_HEAD_COMMIT_INVALID",
        ) != head_commit
    {
        fail("PRODUCER_COMMIT_INVALID");
    }
    if git_value(root, &["rev-parse", "HEAD"], "PRODUCER_HEAD_COMMIT_INVALID") != head_commit {
        fail("PRODUCER_HEAD_COMMIT_MISMATCH");
    }
    let base_ref = producer_string(worktree, "/base_ref", "INVALID_WORKTREE_RECORD");
    if git_value(root, &["rev-parse", &base_ref], "PRODUCER_BASE_REF_INVALID") != base_commit {
        fail("PRODUCER_BASE_REF_MISMATCH");
    }
    let ancestry = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "merge-base",
            "--is-ancestor",
        ])
        .args([base_commit.as_str(), head_commit.as_str()])
        .status()
        .unwrap_or_else(|_| fail("PRODUCER_VCS_UNAVAILABLE"));
    if !ancestry.success() {
        fail("PRODUCER_BASE_NOT_ANCESTOR");
    }
    for path in [
        "/reproduction_id",
        "/issue_id",
        "/module_id",
        "/base_commit",
    ] {
        producer_string(reproduction, path, "INVALID_REPRODUCTION_RECORD");
    }
    if producer_string(reproduction, "/module_id", "INVALID_REPRODUCTION_RECORD") != module_id
        || producer_issue(reproduction, "/issue_id", "INVALID_REPRODUCTION_RECORD")
            != worktree_issue
        || producer_string(reproduction, "/base_commit", "INVALID_REPRODUCTION_RECORD")
            != base_commit
    {
        fail("PRODUCER_REPRODUCTION_MISMATCH");
    }
    for path in [
        "/issue_id",
        "/experiment_id",
        "/phase",
        "/kind",
        "/source_commit",
        "/scope_hash",
        "/scope/module_id",
        "/producer/adapter",
        "/producer/identity",
    ] {
        producer_string(baseline, path, "INVALID_BASELINE_EVIDENCE");
    }
    if producer_issue(baseline, "/issue_id", "INVALID_BASELINE_EVIDENCE") != worktree_issue
        || producer_string(baseline, "/scope/module_id", "INVALID_BASELINE_EVIDENCE") != module_id
        || producer_string(baseline, "/scope_hash", "INVALID_BASELINE_EVIDENCE")
            != producer_string(worktree, "/scope_hash", "INVALID_WORKTREE_RECORD")
        || producer_string(baseline, "/source_commit", "INVALID_BASELINE_EVIDENCE") != base_commit
        || producer_string(baseline, "/phase", "INVALID_BASELINE_EVIDENCE")
            != "baseline_reproduction"
        || !matches!(
            producer_string(baseline, "/kind", "INVALID_BASELINE_EVIDENCE").as_str(),
            "red_test" | "sample_replay" | "gate" | "runtime"
        )
    {
        fail("PRODUCER_BASELINE_MISMATCH");
    }
    let (program, args, working_directory, expected_status, expected_error_token) =
        producer_command(baseline);
    let command_declaration = serde_json::json!({
        "program": program,
        "args": args,
        "working_directory": working_directory,
        "expected_exit_status": expected_status,
        "expected_error_token": expected_error_token
    });
    let input_hashes = vec![sha256(&canonical(&command_declaration))];
    let current_root = root
        .canonicalize()
        .unwrap_or_else(|_| fail("PRODUCER_WORKTREE_UNAVAILABLE"));
    let worktree_id = producer_stable_id(
        "worktree",
        &serde_json::json!({
            "root": current_root,
            "module_id": module_id,
            "issue_id": worktree_issue,
            "base_commit": base_commit,
            "head_commit": head_commit,
            "branch": current_branch,
            "scope_hash": expected_scope_hash
        }),
    );
    let reproduction_id = producer_stable_id(
        "reproduction",
        &serde_json::json!({
            "worktree_id": worktree_id,
            "input_hashes": input_hashes,
            "error_token": expected_error_token
        }),
    );
    let baseline_id = producer_stable_id(
        "baseline",
        &serde_json::json!({
            "reproduction_id": reproduction_id,
            "source_commit": base_commit,
            "input_hashes": input_hashes,
            "command": command_declaration
        }),
    );
    let targets = producer_record_targets(root, module_id, &input, &baseline_id);
    // Validate the declaration before creating a temporary worktree. The
    // baseline checkout must be disposable even when its directory is
    // malformed or absent at the declared source commit.
    let _candidate_command_directory =
        producer_command_dir(root, &working_directory).unwrap_or_else(|error| fail(error));
    let baseline_started_at = Utc::now();
    let baseline_root = producer_baseline_worktree(root, &base_commit);
    match producer_baseline_git_value(&baseline_root, &["rev-parse", "HEAD"]) {
        Ok(head) if head == base_commit => {}
        Ok(_) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail("BASELINE_WORKTREE_COMMIT_MISMATCH");
        }
        Err(error) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail(error);
        }
    }
    let command_directory = match producer_command_dir(&baseline_root, &working_directory) {
        Ok(path) => path,
        Err(error) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail(error);
        }
    };
    let output = match Command::new(&program)
        .args(&args)
        .current_dir(&command_directory)
        .output()
    {
        Ok(output) => output,
        Err(_) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail("BASELINE_COMMAND_FAILED");
        }
    };
    let actual_status = output.status.code().unwrap_or(-1);
    if actual_status != expected_status {
        remove_producer_baseline_worktree(root, &baseline_root);
        fail(format!(
            "BASELINE_COMMAND_STATUS_MISMATCH:expected={}:actual={}",
            expected_status, actual_status
        ));
    }
    match producer_baseline_git_value(&baseline_root, &["rev-parse", "HEAD"]) {
        Ok(head) if head == base_commit => {}
        Ok(_) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail("BASELINE_COMMAND_CHANGED_COMMIT");
        }
        Err(error) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail(error);
        }
    }
    let output_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output_text.contains(&expected_error_token) {
        remove_producer_baseline_worktree(root, &baseline_root);
        fail("BASELINE_ERROR_TOKEN_MISSING");
    }
    match producer_baseline_git_value(
        &baseline_root,
        &["status", "--porcelain", "--untracked-files=all"],
    ) {
        Ok(status) if status.is_empty() => {}
        Ok(_) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail("BASELINE_COMMAND_DIRTY_WORKTREE");
        }
        Err(error) => {
            remove_producer_baseline_worktree(root, &baseline_root);
            fail(error);
        }
    }
    remove_producer_baseline_worktree(root, &baseline_root);
    let output_hash = sha256(&format!(
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ));
    let bug_triage = worktree.get("bug_triage");
    if let Some(recovered) =
        producer_record_transaction_recover(root, module_id, &input_hash, |recovered| {
            assert_recovered_record_bindings(
                recovered,
                module_id,
                &worktree_issue,
                &base_ref,
                &base_commit,
                &head_commit,
                &current_branch,
                &expected_scope_hash,
                &worktree_id,
                &reproduction_id,
                &baseline_id,
                &input_hashes,
                &command_declaration,
                actual_status,
                &output_hash,
                bug_triage,
            );
        })
    {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": true,
                "module_id": module_id,
                "goal_id": input["goal_id"],
                "recovered": true,
                "records": recovered.iter().map(|(target, record)| serde_json::json!({
                    "path": target.strip_prefix(root).unwrap_or(target).display().to_string(),
                    "id": record.get("worktree_id").or_else(|| record.get("reproduction_id")).or_else(|| record.get("evidence_id"))
                })).collect::<Vec<_>>()
            }))
            .unwrap()
        );
        return;
    }
    for target in [
        records_root.join(module_record_name("worktree-record", module_id)),
        records_root.join(module_record_name("reproduction-record", module_id)),
    ] {
        assert_no_symlink_components(root, &target, "record_control");
        if target.exists() {
            fail(format!(
                "LIFECYCLE_RECORD_EXISTS:{}",
                target.strip_prefix(root).unwrap_or(&target).display()
            ));
        }
    }
    let mut observed_worktree = worktree.clone();
    observed_worktree["worktree_id"] = Value::String(worktree_id);
    observed_worktree["base_commit"] = Value::String(base_commit.clone());
    observed_worktree["head_commit"] = Value::String(head_commit.clone());
    observed_worktree["branch"] = Value::String(current_branch);
    observed_worktree["initial_clean"] = Value::Bool(true);
    observed_worktree["final_clean"] = Value::Bool(true);
    observed_worktree["created_at"] = Value::String(baseline_started_at.to_rfc3339());
    if let Some(observed_bug_triage) = worktree.get("bug_triage") {
        if !observed_bug_triage.is_object() {
            fail("INVALID_BUG_TRIAGE");
        }
        observed_worktree["bug_triage"] = observed_bug_triage.clone();
        observed_worktree["bug_triage_query_binding"] =
            Value::String(sha256(&canonical(&serde_json::json!({
                "issue_id": worktree_issue,
                "query": observed_bug_triage["query"],
                "mode": observed_bug_triage["mode"],
                "reopened_from_issue_id": observed_bug_triage["reopened_from_issue_id"]
            }))));
    }
    let mut observed_reproduction = reproduction.clone();
    observed_reproduction["reproduction_id"] = Value::String(reproduction_id);
    observed_reproduction["worktree_id"] = observed_worktree["worktree_id"].clone();
    observed_reproduction["input_hashes"] = serde_json::json!(input_hashes);
    observed_reproduction["baseline_evidence_id"] = Value::String(baseline_id.clone());
    observed_reproduction["first_divergence"] =
        Value::String(format!("baseline_error_token:{}", expected_error_token));
    observed_reproduction["base_commit"] = Value::String(base_commit);
    observed_reproduction["result"] = Value::String("reproduced".into());
    observed_reproduction["created_at"] = Value::String(Utc::now().to_rfc3339());
    let mut observed_baseline = baseline.clone();
    observed_baseline["source_commit"] = Value::String(producer_string(
        &worktree,
        "/base_commit",
        "INVALID_WORKTREE_RECORD",
    ));
    observed_baseline["evidence_id"] = Value::String(baseline_id);
    observed_baseline["input_hashes"] = serde_json::json!(input_hashes);
    observed_baseline["producer"] = serde_json::json!({
        "adapter": "appsdk",
        "identity": "appsdk-lifecycle-record-producer"
    });
    observed_baseline["result"] = Value::String("pass".into());
    observed_baseline["command"] = command_declaration;
    observed_baseline["exit_status"] = Value::Number(actual_status.into());
    observed_baseline["output_hash"] = Value::String(output_hash);
    observed_baseline["created_at"] = Value::String(Utc::now().to_rfc3339());
    observed_baseline["expires_at"] =
        Value::String((Utc::now() + chrono::Duration::hours(24)).to_rfc3339());
    let targets = vec![
        (targets[0].0.clone(), observed_worktree),
        (targets[1].0.clone(), observed_reproduction),
        (targets[2].0.clone(), observed_baseline),
    ];
    assert_produced_record_shapes(&targets, module_id);
    producer_commit_records(root, module_id, &input_hash, &targets);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "ok": true,
            "module_id": module_id,
            "goal_id": input["goal_id"],
            "records": targets.iter().map(|(target, record)| serde_json::json!({
                "path": target.strip_prefix(root).unwrap_or(target).display().to_string(),
                "id": record.get("worktree_id").or_else(|| record.get("reproduction_id")).or_else(|| record.get("evidence_id"))
            })).collect::<Vec<_>>()
        }))
        .unwrap()
    );
}

fn lifecycle_chain_input(root: &Path, input_path: &str, phase: &str) -> Value {
    let input_file = producer_input_path(root, input_path);
    let input: Value = serde_json::from_str(
        &fs::read_to_string(input_file).unwrap_or_else(|_| fail("PRODUCER_INPUT_READ_FAILED")),
    )
    .unwrap_or_else(|_| fail("INVALID_PRODUCER_INPUT"));
    if !input.is_object() {
        fail("INVALID_PRODUCER_INPUT");
    }
    input
        .get(phase)
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| {
            fail(format!(
                "PRODUCER_{}_INPUT_MISSING",
                phase.to_ascii_uppercase()
            ))
        })
}

fn lifecycle_chain_required_array(value: &Value, path: &str, error: &str) -> Vec<String> {
    let values = value
        .pointer(path)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| fail(error));
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let id = value
            .as_str()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| fail(error));
        result.push(id.to_string());
    }
    result
}

fn lifecycle_chain_candidate(root: &Path, module_id: &str) -> (Value, Value, Value, Value) {
    let candidate_name = module_record_name("fix-candidate-record", module_id);
    let candidate = read_record(root, &candidate_name);
    let validation = read_record(
        root,
        &module_record_name("pre-review-validation-record", module_id),
    );
    let reproduction = read_record(root, &module_record_name("reproduction-record", module_id));
    let worktree = read_record(root, &module_record_name("worktree-record", module_id));
    if producer_string(&candidate, "/module_id", &candidate_name) != module_id
        || producer_string(
            &validation,
            "/module_id",
            "pre-review-validation-record.json",
        ) != module_id
        || producer_string(&reproduction, "/module_id", "reproduction-record.json") != module_id
        || producer_string(&worktree, "/module_id", "worktree-record.json") != module_id
    {
        fail("LIFECYCLE_CHAIN_MODULE_MISMATCH");
    }
    (worktree, reproduction, candidate, validation)
}

fn lifecycle_chain_write_record(root: &Path, module_id: &str, kind: &str, record: &Value) {
    let target = root
        .join(".appsdk")
        .join("records")
        .join(module_record_name(kind, module_id));
    assert_no_symlink_components(root, &target, "lifecycle_chain_record");
    if target.exists() {
        fail(format!(
            "LIFECYCLE_RECORD_EXISTS:{}",
            target.strip_prefix(root).unwrap_or(&target).display()
        ));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|_| fail("LIFECYCLE_CHAIN_RECORD_WRITE_FAILED"));
    }
    producer_durable_json(&target, record, "LIFECYCLE_CHAIN_RECORD_WRITE_FAILED");
}

fn lifecycle_chain_validate_evidence(
    root: &Path,
    module_id: &str,
    evidence_id: &str,
    issue_id: &str,
    scope_hash: &str,
    source_commit: &str,
) -> Value {
    let evidence = evidence_by_id(root, module_id, evidence_id);
    assert_evidence_record(&evidence, evidence_id, Utc::now());
    if producer_string(&evidence, "/evidence_id", evidence_id) != evidence_id
        || producer_string(&evidence, "/issue_id", evidence_id) != issue_id
        || producer_string(&evidence, "/scope/module_id", evidence_id) != module_id
        || producer_string(&evidence, "/scope_hash", evidence_id) != scope_hash
        || producer_string(&evidence, "/source_commit", evidence_id) != source_commit
        || producer_string(&evidence, "/result", evidence_id) != "pass"
    {
        fail("LIFECYCLE_CHAIN_EVIDENCE_MISMATCH");
    }
    evidence
}

fn lifecycle_chain_promotion_id(issue_id: &str, module_id: &str, candidate_id: &str) -> String {
    producer_stable_id(
        "promotion",
        &serde_json::json!({
            "issue_id": issue_id,
            "module_id": module_id,
            "fix_candidate_id": candidate_id
        }),
    )
}

fn lifecycle_chain_architecture(root: &Path, module_id: &str, input_path: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_declared_contracts(root, &project, true);
    assert_goal_confirmed(root);
    assert_lifecycle_producer_map_binding(root, &project, module_id);
    let observation = lifecycle_chain_input(root, input_path, "architecture");
    let (worktree, _reproduction, candidate, _validation) =
        lifecycle_chain_candidate(root, module_id);
    let artifact = read_module_artifact(root, &project, module_id);
    module_artifact_matches_project(
        project
            .get("modules")
            .and_then(Value::as_array)
            .and_then(|modules| {
                modules.iter().find(|module| {
                    module.get("module_id").and_then(Value::as_str) == Some(module_id)
                })
            })
            .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id))),
        &artifact,
    );
    assert_pre_review_validation_gate(root, module_id, &artifact);
    let issue_id = producer_string(&worktree, "/issue_id", "worktree-record.json");
    let candidate_id =
        producer_string(&candidate, "/fix_candidate_id", "fix-candidate-record.json");
    let candidate_commit = producer_string(&candidate, "/head_commit", "fix-candidate-record.json");
    let candidate_tree = producer_string(&candidate, "/tree_hash", "fix-candidate-record.json");
    let scope_hash = producer_string(&candidate, "/scope_hash", "fix-candidate-record.json");
    assert_lifecycle_chain_candidate_at_head(
        root,
        &project,
        module_id,
        &candidate_commit,
        &candidate_tree,
    );
    let evidence_ids = lifecycle_chain_required_array(
        &observation,
        "/evidence_ids",
        "ARCHITECTURE_REVIEW_EVIDENCE_MISSING",
    );
    let review_time = Utc::now();
    for id in &evidence_ids {
        let evidence = lifecycle_chain_validate_evidence(
            root,
            module_id,
            id,
            &issue_id,
            &scope_hash,
            &candidate_commit,
        );
        if record_time(&evidence, id) > review_time {
            fail("ARCHITECTURE_REVIEW_EVIDENCE_FUTURE");
        }
    }
    let reviewer = observation
        .get("reviewer")
        .filter(|value| {
            value
                .get("adapter")
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty())
                && value
                    .get("identity")
                    .and_then(Value::as_str)
                    .is_some_and(|v| !v.is_empty())
        })
        .cloned()
        .unwrap_or_else(|| fail("ARCHITECTURE_REVIEWER_MISSING"));
    let project_bindings = observation.get("project_bindings").map(|value| {
        if !value.is_object() {
            fail("ARCHITECTURE_REVIEW_PROJECT_BINDINGS_INVALID");
        }
        value.clone()
    });
    let verdict = producer_string(
        &observation,
        "/verdict",
        "ARCHITECTURE_REVIEW_VERDICT_MISSING",
    );
    if !matches!(
        verdict.as_str(),
        "pass" | "fail" | "new_version_required" | "manual_auth_required"
    ) {
        fail("ARCHITECTURE_REVIEW_VERDICT_INVALID");
    }
    let promotion_id = lifecycle_chain_promotion_id(&issue_id, module_id, &candidate_id);
    let review_evidence_ids = serde_json::json!(evidence_ids);
    let review_id = producer_stable_id(
        "review",
        &lifecycle_chain_review_identity(
            &promotion_id,
            &candidate_id,
            &reviewer,
            &verdict,
            &review_evidence_ids,
            project_bindings.as_ref(),
        ),
    );
    let mut review = serde_json::json!({
        "review_id": review_id,
        "review_kind": "architecture",
        "issue_id": issue_id,
        "promotion_id": promotion_id,
        "fix_candidate_id": candidate_id,
        "pre_review_validation_id": producer_string(&_validation, "/validation_id", "pre-review-validation-record.json"),
        "reviewer": reviewer,
        "verdict": verdict,
        "evidence_ids": evidence_ids,
        "reviewed_commit": candidate_commit,
        "reviewed_tree_hash": candidate_tree,
        "reviewed_diff_hash": producer_string(&candidate, "/diff_hash", "fix-candidate-record.json"),
        "reviewed_artifact_hash": producer_string(&artifact, "/artifact_hash", "module-artifact"),
        "reviewed_scope_hash": scope_hash,
        "resource_map_hash": file_sha256(&root.join(".appsdk/maps/resource-map.json"), "resource-map.json"),
        "function_map_hash": file_sha256(&root.join(".appsdk/maps/function-map.json"), "function-map.json"),
        "mainline_call_map_hash": file_sha256(&root.join(".appsdk/maps/mainline-call-map.json"), "mainline-call-map.json"),
        "verification_map_hash": file_sha256(&root.join(".appsdk/maps/verification-map.json"), "verification-map.json"),
        "created_at": review_time.to_rfc3339()
    });
    if let Some(bindings) = project_bindings {
        review["project_bindings"] = bindings;
    }
    for path in [
        "/review_id",
        "/promotion_id",
        "/fix_candidate_id",
        "/pre_review_validation_id",
        "/reviewed_commit",
        "/reviewed_tree_hash",
        "/reviewed_diff_hash",
        "/reviewed_artifact_hash",
        "/reviewed_scope_hash",
        "/resource_map_hash",
        "/function_map_hash",
        "/mainline_call_map_hash",
        "/verification_map_hash",
        "/created_at",
    ] {
        producer_string(&review, path, "ARCHITECTURE_REVIEW_RECORD_INVALID");
    }
    lifecycle_chain_write_record(root, module_id, "review-record", &review);
    println!("{}", serde_json::to_string_pretty(&review).unwrap());
}

fn lifecycle_chain_effectiveness(root: &Path, module_id: &str, input_path: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_declared_contracts(root, &project, true);
    assert_goal_confirmed(root);
    let observation = lifecycle_chain_input(root, input_path, "effectiveness");
    let (worktree, reproduction, candidate, _validation) =
        lifecycle_chain_candidate(root, module_id);
    let review_name = module_record_name("review-record", module_id);
    let review = read_record(root, &review_name);
    let artifact = read_module_artifact(root, &project, module_id);
    assert_fix_architecture_gate(root, module_id, &artifact);
    let issue_id = producer_string(&worktree, "/issue_id", "worktree-record.json");
    let candidate_id =
        producer_string(&candidate, "/fix_candidate_id", "fix-candidate-record.json");
    let candidate_commit = producer_string(&candidate, "/head_commit", "fix-candidate-record.json");
    let candidate_tree = producer_string(&candidate, "/tree_hash", "fix-candidate-record.json");
    let scope_hash = producer_string(&candidate, "/scope_hash", "fix-candidate-record.json");
    assert_lifecycle_chain_candidate_at_head(
        root,
        &project,
        module_id,
        &candidate_commit,
        &candidate_tree,
    );
    let fixed_id = producer_string(
        &observation,
        "/fixed_replay_evidence_id",
        "EFFECTIVENESS_FIXED_REPLAY_MISSING",
    );
    let positive_ids = lifecycle_chain_required_array(
        &observation,
        "/positive_evidence_ids",
        "EFFECTIVENESS_POSITIVE_EVIDENCE_MISSING",
    );
    let negative_ids = lifecycle_chain_required_array(
        &observation,
        "/negative_evidence_ids",
        "EFFECTIVENESS_NEGATIVE_EVIDENCE_MISSING",
    );
    let blackbox_ids = lifecycle_chain_required_array(
        &observation,
        "/blackbox_evidence_ids",
        "EFFECTIVENESS_BLACKBOX_EVIDENCE_MISSING",
    );
    let mut all_ids = vec![fixed_id.clone()];
    all_ids.extend(positive_ids.iter().cloned());
    all_ids.extend(negative_ids.iter().cloned());
    all_ids.extend(blackbox_ids.iter().cloned());
    let effectiveness_time = Utc::now();
    let mut phases = std::collections::HashSet::new();
    for id in &all_ids {
        let evidence = lifecycle_chain_validate_evidence(
            root,
            module_id,
            id,
            &issue_id,
            &scope_hash,
            &candidate_commit,
        );
        if evidence.get("input_hashes") != reproduction.get("input_hashes") {
            fail("EFFECTIVENESS_INPUT_MISMATCH");
        }
        if record_time(&evidence, id) > effectiveness_time {
            fail("EFFECTIVENESS_EVIDENCE_FUTURE");
        }
        phases.insert(producer_string(&evidence, "/phase", id));
    }
    if !phases.contains("positive_intervention")
        || !phases.contains("negative_intervention")
        || (!phases.contains("post_architecture_effectiveness")
            && !phases.contains("deployed_blackbox"))
    {
        fail("EFFECTIVENESS_REQUIRED_PHASE_MISSING");
    }
    let review_id = producer_string(&review, "/review_id", &review_name);
    let effectiveness_id = producer_stable_id(
        "effectiveness",
        &serde_json::json!({"fix_candidate_id": candidate_id, "architecture_review_id": review_id, "evidence_ids": all_ids}),
    );
    let effectiveness = serde_json::json!({
        "effectiveness_id": effectiveness_id,
        "issue_id": issue_id,
        "module_id": module_id,
        "fix_candidate_id": candidate_id,
        "architecture_review_id": review_id,
        "reviewed_commit": candidate_commit,
        "reviewed_tree_hash": candidate_tree,
        "reproduction_input_hashes": reproduction["input_hashes"].clone(),
        "baseline_evidence_id": producer_string(&reproduction, "/baseline_evidence_id", "reproduction-record.json"),
        "fixed_replay_evidence_id": fixed_id,
        "positive_evidence_ids": positive_ids,
        "negative_evidence_ids": negative_ids,
        "blackbox_evidence_ids": blackbox_ids,
        "source_unchanged_since_review": true,
        "result": "pass",
        "created_at": effectiveness_time.to_rfc3339()
    });
    for path in [
        "/effectiveness_id",
        "/issue_id",
        "/module_id",
        "/fix_candidate_id",
        "/architecture_review_id",
        "/reviewed_commit",
        "/reviewed_tree_hash",
        "/baseline_evidence_id",
        "/fixed_replay_evidence_id",
        "/created_at",
    ] {
        producer_string(&effectiveness, path, "EFFECTIVENESS_RECORD_INVALID");
    }
    lifecycle_chain_write_record(root, module_id, "effectiveness-record", &effectiveness);
    println!("{}", serde_json::to_string_pretty(&effectiveness).unwrap());
}

fn assert_lifecycle_chain_promotion_gates(gates: &[Value]) {
    let verification_map: Value =
        serde_json::from_str(canonical_governance_map("verification-map.json"))
            .unwrap_or_else(|_| fail("PROMOTION_VERIFICATION_MAP_INVALID"));
    let expected = record_array(&verification_map, "/gates", "verification-map.json")
        .iter()
        .filter(|gate| {
            gate.get("required_for")
                .and_then(Value::as_array)
                .is_some_and(|uses| {
                    uses.iter()
                        .any(|use_case| use_case.as_str() == Some("promotion"))
                })
        })
        .map(|gate| record_str(gate, "/gate_id", "verification-map.json"))
        .collect::<Vec<_>>();
    if expected.is_empty() {
        fail("PROMOTION_GATES_UNDECLARED");
    }
    let mut actual = std::collections::HashSet::new();
    for gate in gates {
        let gate_id = record_str(gate, "/gate_id", "PROMOTION_GATE_INVALID");
        if !actual.insert(gate_id) || gate.get("result").and_then(Value::as_str) != Some("pass") {
            fail("PROMOTION_GATE_INVALID");
        }
        if gate
            .get("producer")
            .and_then(Value::as_str)
            .filter(|producer| !producer.is_empty())
            .is_none()
        {
            fail("PROMOTION_GATE_INVALID");
        }
    }
    if actual.len() != expected.len() || expected.iter().any(|gate_id| !actual.contains(gate_id)) {
        fail("PROMOTION_GATE_SET_MISMATCH");
    }
}

fn lifecycle_chain_merge(root: &Path, module_id: &str, input_path: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_declared_contracts(root, &project, true);
    assert_goal_confirmed(root);
    let observation = lifecycle_chain_input(root, input_path, "merge");
    let (worktree, _reproduction, candidate, _validation) =
        lifecycle_chain_candidate(root, module_id);
    let effectiveness = read_record(root, &module_record_name("effectiveness-record", module_id));
    assert_fix_effectiveness_gate(root, module_id);
    let issue_id = producer_string(&worktree, "/issue_id", "worktree-record.json");
    let candidate_id =
        producer_string(&candidate, "/fix_candidate_id", "fix-candidate-record.json");
    let candidate_commit = producer_string(&candidate, "/head_commit", "fix-candidate-record.json");
    let candidate_tree = producer_string(&candidate, "/tree_hash", "fix-candidate-record.json");
    let mainline_ref = producer_string(&observation, "/mainline_ref", "MERGE_MAINLINE_REF_MISSING");
    let merge_commit = git_value(root, &["rev-parse", "HEAD"], "MERGE_COMMIT_UNAVAILABLE");
    let merged_tree = git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", merge_commit)],
        "MERGE_TREE_UNAVAILABLE",
    );
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{commit}}", mainline_ref)],
        "MERGE_MAINLINE_REF_MISSING",
    ) != merge_commit
    {
        fail("MERGE_MAINLINE_HEAD_MISMATCH");
    }
    if git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "FIX_CANDIDATE_COMMIT_MISSING",
    ) != candidate_tree
        || !Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "merge-base",
                "--is-ancestor",
                candidate_commit.as_str(),
                merge_commit.as_str(),
            ])
            .status()
            .is_ok_and(|status| status.success())
    {
        fail("MERGE_CANDIDATE_IDENTITY_MISMATCH");
    }
    let change_identity = if candidate_commit == merge_commit {
        "exact"
    } else {
        let requested = producer_string(
            &observation,
            "/change_identity",
            "MERGE_CHANGE_IDENTITY_MISSING",
        );
        if requested != "tested_integration_exact" {
            fail("MERGE_CHANGE_IDENTITY_REQUIRED");
        }
        "tested_integration_exact"
    };
    let merge_time = Utc::now();
    let merge_id = producer_stable_id(
        "merge",
        &serde_json::json!({"fix_candidate_id": candidate_id, "effectiveness_id": effectiveness["effectiveness_id"], "merge_commit": merge_commit, "mainline_ref": mainline_ref}),
    );
    let merge = serde_json::json!({
        "merge_id": merge_id,
        "issue_id": issue_id,
        "module_id": module_id,
        "fix_candidate_id": candidate_id,
        "effectiveness_id": effectiveness["effectiveness_id"].clone(),
        "mainline_ref": mainline_ref,
        "candidate_commit": candidate_commit,
        "merge_commit": merge_commit,
        "candidate_tree_hash": candidate_tree,
        "merged_tree_hash": merged_tree,
        "change_identity": change_identity,
        "result": "pass",
        "created_at": merge_time.to_rfc3339()
    });
    lifecycle_chain_write_record(root, module_id, "merge-record", &merge);
    println!("{}", serde_json::to_string_pretty(&merge).unwrap());
}

fn lifecycle_chain_promotion(root: &Path, module_id: &str, input_path: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_declared_contracts(root, &project, true);
    assert_goal_confirmed(root);
    assert_lifecycle_producer_map_binding(root, &project, module_id);
    let observation = lifecycle_chain_input(root, input_path, "promotion");
    let (worktree, reproduction, candidate, _validation) =
        lifecycle_chain_candidate(root, module_id);
    let review = read_record(root, &module_record_name("review-record", module_id));
    let effectiveness = read_record(root, &module_record_name("effectiveness-record", module_id));
    let merge = read_record(root, &module_record_name("merge-record", module_id));
    let artifact = read_module_artifact(root, &project, module_id);
    let issue_id = producer_string(&worktree, "/issue_id", "worktree-record.json");
    let candidate_id =
        producer_string(&candidate, "/fix_candidate_id", "fix-candidate-record.json");
    let merge_commit = producer_string(&merge, "/merge_commit", "merge-record.json");
    let artifact_hash = producer_string(&artifact, "/artifact_hash", "module-artifact");
    let scope_hash = producer_string(&candidate, "/scope_hash", "fix-candidate-record.json");
    let public_api_hash = producer_string(&artifact, "/public_api_hash", "module-artifact");
    if git_value(root, &["rev-parse", "HEAD"], "PROMOTION_HEAD_UNAVAILABLE") != merge_commit {
        fail("PROMOTION_MERGE_HEAD_MISMATCH");
    }
    let experiment_id = producer_string(
        &observation,
        "/experiment_id",
        "PROMOTION_EXPERIMENT_MISSING",
    );
    let new_version = producer_string(
        &observation,
        "/new_active_version",
        "PROMOTION_NEW_VERSION_MISSING",
    );
    assert_version(&new_version, "INVALID_ACTIVE_VERSION");
    let previous = if let Some(base) = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|m| m.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .and_then(|m| m.get("version_base"))
        .filter(|value| !value.is_null())
    {
        Value::String(
            record_str(base, "/previous_active_version", "module-version-base").to_string(),
        )
    } else {
        observation
            .get("previous_active_version")
            .filter(|value| value.is_null() || value.as_str().is_some())
            .cloned()
            .unwrap_or_else(|| fail("PROMOTION_PREVIOUS_VERSION_MISSING"))
    };
    let compatibility = producer_string(
        &observation,
        "/compatibility_level",
        "PROMOTION_COMPATIBILITY_MISSING",
    );
    if !matches!(
        compatibility.as_str(),
        "compatible" | "migration_required" | "breaking"
    ) {
        fail("PROMOTION_COMPATIBILITY_INVALID");
    }
    let evidence_ids =
        lifecycle_chain_required_array(&observation, "/evidence_ids", "PROMOTION_EVIDENCE_MISSING");
    for id in &evidence_ids {
        lifecycle_chain_validate_evidence(
            root,
            module_id,
            id,
            &issue_id,
            &scope_hash,
            &merge_commit,
        );
    }
    let gates = observation
        .get("required_gate_results")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .cloned()
        .unwrap_or_else(|| fail("PROMOTION_GATES_MISSING"));
    assert_lifecycle_chain_promotion_gates(&gates);
    for gate in &gates {
        if producer_string(gate, "/gate_id", "PROMOTION_GATE_INVALID") == ""
            || producer_string(gate, "/producer", "PROMOTION_GATE_INVALID") == ""
            || producer_string(gate, "/result", "PROMOTION_GATE_INVALID") != "pass"
        {
            fail("PROMOTION_GATE_INVALID");
        }
    }
    let cleanup_id = producer_string(
        &observation,
        "/playground_cleanup_record_id",
        "PROMOTION_CLEANUP_MISSING",
    );
    assert_fix_merge_gate(root, module_id);
    let cleanup = read_record(root, &format!("playground-cleanup-{}.json", cleanup_id));
    if producer_string(&cleanup, "/cleanup_id", "playground-cleanup-record") != cleanup_id {
        fail("PROMOTION_CLEANUP_MISMATCH");
    }
    let promotion_id = lifecycle_chain_promotion_id(&issue_id, module_id, &candidate_id);
    let promotion_time = Utc::now();
    let promotion = serde_json::json!({
        "promotion_id": promotion_id,
        "issue_id": issue_id,
        "experiment_id": experiment_id,
        "module_id": module_id,
        "worktree_record_id": worktree["worktree_id"].clone(),
        "reproduction_record_id": reproduction["reproduction_id"].clone(),
        "fix_candidate_id": candidate_id,
        "architecture_review_id": review["review_id"].clone(),
        "effectiveness_record_id": effectiveness["effectiveness_id"].clone(),
        "merge_record_id": merge["merge_id"].clone(),
        "base_commit": worktree["base_commit"].clone(),
        "candidate_commit": candidate["head_commit"].clone(),
        "merged_commit": merge_commit.clone(),
        "source_commit": merge_commit,
        "previous_active_version": previous,
        "new_active_version": new_version,
        "artifact_hash": artifact_hash,
        "scope_hash": scope_hash,
        "public_api_hash": public_api_hash,
        "review_id": review["review_id"].clone(),
        "evidence_ids": evidence_ids,
        "required_gate_results": gates,
        "change_set_id": producer_string(&observation, "/change_set_id", "PROMOTION_CHANGE_SET_MISSING"),
        "compatibility_level": compatibility,
        "root_cause": producer_string(&observation, "/root_cause", "PROMOTION_ROOT_CAUSE_MISSING"),
        "design_id": producer_string(&observation, "/design_id", "PROMOTION_DESIGN_MISSING"),
        "change_reason_comment": producer_string(&observation, "/change_reason_comment", "PROMOTION_REASON_MISSING"),
        "playground_cleanup_record_id": cleanup_id,
        "created_at": promotion_time.to_rfc3339()
    });
    lifecycle_chain_write_record(root, module_id, "promotion-record", &promotion);
    println!("{}", serde_json::to_string_pretty(&promotion).unwrap());
}

fn produce_lifecycle_chain(root: &Path, module_id: &str, phase: &str, input_path: &str) {
    if !matches!(
        phase,
        "architecture" | "effectiveness" | "merge" | "promotion"
    ) {
        fail("PRODUCER_PHASE_INVALID");
    }
    let _producer_lock = producer_lock(root);
    match phase {
        "architecture" => lifecycle_chain_architecture(root, module_id, input_path),
        "effectiveness" => lifecycle_chain_effectiveness(root, module_id, input_path),
        "merge" => lifecycle_chain_merge(root, module_id, input_path),
        "promotion" => lifecycle_chain_promotion(root, module_id, input_path),
        _ => unreachable!(),
    }
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

fn assert_record_schema(
    evidence: &Value,
    review: &Value,
    promotion: &Value,
    allow_legacy_rehydrate_bindings: bool,
) {
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
    if !allow_legacy_rehydrate_bindings {
        assert_lifecycle_chain_review_identity(review);
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

fn assert_worktree_candidate_ancestry(root: &Path, worktree_head: &str, candidate_commit: &str) {
    if !Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "merge-base",
            "--is-ancestor",
            worktree_head,
            candidate_commit,
        ])
        .status()
        .is_ok_and(|status| status.success())
    {
        fail("FIX_REPRODUCTION_GRAPH_MISMATCH");
    }
}

fn assert_lifecycle_chain_candidate_at_head(
    root: &Path,
    project: &Value,
    module_id: &str,
    candidate_commit: &str,
    candidate_tree: &str,
) {
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
        "PRODUCER_CANDIDATE_TREE_UNAVAILABLE",
    ) != candidate_tree
    {
        fail("LIFECYCLE_CHAIN_CANDIDATE_DRIFT");
    }
    let head_commit = git_value(
        root,
        &["rev-parse", "HEAD"],
        "PRODUCER_HEAD_COMMIT_UNAVAILABLE",
    );
    if !Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "merge-base",
            "--is-ancestor",
            candidate_commit,
            &head_commit,
        ])
        .status()
        .is_ok_and(|status| status.success())
    {
        fail("LIFECYCLE_CHAIN_CANDIDATE_DRIFT");
    }
    assert_candidate_source_identity(root, module, candidate_commit);
    let git_root = PathBuf::from(git_value(
        root,
        &["rev-parse", "--show-toplevel"],
        "CANDIDATE_SOURCE_GIT_UNAVAILABLE",
    ));
    let project_root = root
        .canonicalize()
        .unwrap_or_else(|_| fail("CANDIDATE_SOURCE_GIT_UNAVAILABLE"));
    let git_root = git_root
        .canonicalize()
        .unwrap_or_else(|_| fail("CANDIDATE_SOURCE_GIT_UNAVAILABLE"));
    let project_relative = project_root
        .strip_prefix(&git_root)
        .unwrap_or_else(|_| fail("CANDIDATE_SOURCE_GIT_UNAVAILABLE"));
    let records_prefix = if project_relative.as_os_str().is_empty() {
        ".appsdk/records/".to_string()
    } else {
        format!("{}/.appsdk/records/", project_relative.to_string_lossy())
    };
    let changed = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--name-only", candidate_commit, &head_commit])
        .output()
        .unwrap_or_else(|_| fail("CANDIDATE_SOURCE_GIT_UNAVAILABLE"));
    if !changed.status.success()
        || String::from_utf8_lossy(&changed.stdout)
            .lines()
            .any(|path| !path.starts_with(&records_prefix))
    {
        fail("LIFECYCLE_CHAIN_CANDIDATE_DRIFT");
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
    if environment_id.is_empty() || entrypoint.is_empty() {
        fail("DEPLOYMENT_BLACKBOX_RECEIPT_MISSING");
    }
    let mut all_ids = std::collections::HashSet::new();
    let required_operations = module_deployment_operations(module);
    let mut receipt_times = Vec::new();
    for (operation, phase, path) in [
        (
            "install",
            "deployment_install",
            "/deployment/install_receipt_id",
        ),
        (
            "restart",
            "deployment_restart",
            "/deployment/restart_receipt_id",
        ),
    ] {
        // Validate supplied receipts too; optional does not mean silently ignored.
        if required_operations.contains(&operation) || validation.pointer(path).is_some() {
            let id = record_str(&validation, path, &validation_name);
            if !all_ids.insert(id) {
                fail("PRE_REVIEW_EVIDENCE_NOT_DISJOINT");
            }
            receipt_times.push(deployment_receipt_time(
                root,
                module_id,
                id,
                phase,
                operation,
                issue_id,
                scope_hash,
                candidate_commit,
                artifact_hash,
                environment_id,
                entrypoint,
                producer,
            ));
        }
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
    let mut previous_time =
        latest_whitebox.unwrap_or_else(|| fail("MISSING_DEVELOPMENT_WHITEBOX_EVIDENCE"));
    for time in receipt_times {
        if previous_time > time {
            fail("PRE_REVIEW_CAUSAL_ORDER_MISMATCH");
        }
        previous_time = time;
    }
    if record_time(&candidate, &candidate_name)
        > earliest_whitebox.unwrap_or_else(|| fail("MISSING_DEVELOPMENT_WHITEBOX_EVIDENCE"))
        || previous_time
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
    explain_review_admission_preflight(root, module_id, module);
    assert_pre_review_validation_gate(root, module_id, &artifact);
    verify_internal(root, true, false);
    println!(
        "{{\"ok\":true,\"gate\":\"review_admission\",\"module_id\":\"{}\"}}",
        module_id
    );
}

fn explain_review_admission_preflight(root: &Path, module_id: &str, module: &Value) {
    let records_root = root.join(".appsdk").join("records");
    let evidence_root = records_root.join("evidence").join(module_id);
    let mut required = vec![
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
    ];
    let deployment_operations = module_deployment_operations(module);
    if deployment_operations.contains(&"install") {
        required.push((
            "deployment_install",
            "evidence/<module>/install-1.json".to_string(),
            "project::deployment_adapter",
            "install the exact candidate artifact and persist the real receipt",
        ));
    }
    if deployment_operations.contains(&"restart") {
        required.push((
            "deployment_restart",
            "evidence/<module>/restart-1.json".to_string(),
            "project::deployment_adapter",
            "restart the exact installed artifact and persist the real receipt",
        ));
    }
    required.extend([
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
    ]);
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
    assert_worktree_candidate_ancestry(
        root,
        record_str(&worktree, "/head_commit", &worktree_name),
        candidate_commit,
    );
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
    assert_bug_tracker_triage_evidence(&worktree, issue_id, None, true);
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
    if record_str(&candidate, "/scope_hash", &candidate_name) != scope_hash
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
    assert_lifecycle_chain_review_identity_or_frozen_legacy(root, module_id, &review);
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
    let validation_name = module_record_name("pre-review-validation-record", module_id);
    let validation = read_record(root, &validation_name);
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
            || record_time(&evidence, &id) < record_time(&candidate, &candidate_name)
            || record_time(&evidence, &id) > record_time(&effectiveness, &effectiveness_name)
        {
            fail("POST_ARCHITECTURE_EFFECTIVENESS_EVIDENCE_MISMATCH");
        }
        if record_time(&evidence, &id) < record_time(&review, &review_name) {
            let bound_before_review =
                record_array(&candidate, "/verification_evidence_ids", &candidate_name)
                    .iter()
                    .chain(
                        record_array(&validation, "/blackbox_evidence_ids", &validation_name)
                            .iter(),
                    )
                    .any(|value| value.as_str() == Some(id.as_str()));
            if !bound_before_review
                || evidence.get("input_hashes") != reproduction.get("input_hashes")
                || evidence.get("artifact_hash") != validation.get("artifact_hash")
            {
                fail("EFFECTIVENESS_REUSED_EVIDENCE_MISMATCH");
            }
        }
        phases.push(record_str(&evidence, "/phase", &id).to_string());
    }
    for phase in ["positive_intervention", "negative_intervention"] {
        if !phases.iter().any(|value| value == phase) {
            fail(format!("MISSING_EFFECTIVENESS_EVIDENCE_PHASE:{}", phase));
        }
    }
    if !phases.iter().any(|phase| {
        matches!(
            phase.as_str(),
            "post_architecture_effectiveness" | "deployed_blackbox"
        )
    }) {
        fail("MISSING_EFFECTIVENESS_EVIDENCE_PHASE:public_entrypoint");
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
    assert_worktree_candidate_ancestry(
        root,
        record_str(&worktree, "/head_commit", &worktree_name),
        candidate_commit,
    );
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
    if record_str(&candidate, "/scope_hash", &candidate_name) != scope_hash
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
    assert_bug_tracker_solution_evidence(root, issue_id, promotion);
}

fn assert_record_graph(
    root: &Path,
    module_id: Option<&str>,
    artifact: &Value,
    require_freeze: bool,
) {
    assert_record_graph_mode(root, module_id, artifact, require_freeze, true, false);
}

// Frozen rehydration republishes an already accepted historical artifact. It
// must validate the immutable record graph and freeze bindings, but it must
// not re-run delivery-only gates (including current bug-triage evidence) that
// were introduced after the historical producer ran.
fn assert_historical_frozen_record_graph(root: &Path, module_id: &str, artifact: &Value) {
    // Frozen modules are immutable historical publications. Their legacy
    // predecessor binding is not the current development/promotion contract;
    // validate the publication graph without requiring that old Active
    // projection to be present or byte-identical to a later record.
    // The merge/mainline binding remains authoritative and must still be
    // resolved before a historical publication is rehydrated.
    assert_fix_merge_gate(root, module_id);
    assert_record_graph_mode(root, Some(module_id), artifact, true, false, true);
}

fn assert_record_graph_mode(
    root: &Path,
    module_id: Option<&str>,
    artifact: &Value,
    require_freeze: bool,
    enforce_current_lifecycle: bool,
    allow_legacy_rehydrate_bindings: bool,
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
    assert_record_schema(
        &evidence,
        &review,
        &promotion,
        allow_legacy_rehydrate_bindings,
    );
    if enforce_current_lifecycle {
        if let Some(module_id) = module_id {
            assert_fix_lifecycle_graph(root, module_id, &review, &promotion, artifact);
        }
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
        if let Some(version_base) = module.get("version_base").filter(|value| !value.is_null()) {
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
                if !allow_legacy_rehydrate_bindings {
                    fail("PREVIOUS_ACTIVE_MISSING");
                }
                // A legacy frozen checkout may retain the immutable target
                // archive without publishing its predecessor Active
                // projection. Rehydrate validates the target archive and
                // does not invent the missing predecessor.
            }
            if previous_path.is_dir() {
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
                let promotion =
                    read_record(root, &module_record_name("promotion-record", module_id));
                if promotion
                    .pointer("/base_artifact_hash")
                    .and_then(Value::as_str)
                    != Some(previous_hash)
                    && !allow_legacy_rehydrate_bindings
                {
                    fail("PREVIOUS_ACTIVE_HASH_MISMATCH");
                }
                if record_str(&previous_value, "/module_id", "previous_active_artifact")
                    != module_id
                {
                    fail("PREVIOUS_ACTIVE_MODULE_MISSING");
                }
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
    if let Some(module_artifact) = &module_artifact {
        write_module_artifact_value(root, &project, module_id, module_artifact);
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
    // Development artifacts are admissible inputs, never frozen publications.
    module_dependency_hashes(root, &candidate, &candidate["modules"][index], module_id);
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
    let freeze_name = freeze_record_name(module_id);
    let mut freeze = read_record(root, &freeze_name);
    let active_version = record_str(&freeze, "/active_version", &freeze_name);
    let protected_root = contract_root(root, &project, "/governance/protected_root");
    let history_root = protected_root.join("history").join(module_id);
    let archive = if history_root.exists() {
        protected_root
            .join("history-versions")
            .join(module_id)
            .join(active_version)
    } else {
        history_root
    };
    let staging_archive = archive
        .parent()
        .unwrap_or_else(|| fail("PROTECTED_ARCHIVE_FAILED"))
        .join(format!(
            ".{}.staging.{}",
            active_version,
            std::process::id()
        ));
    assert_no_symlink_components(root, &archive, "protected_archive");
    assert_no_symlink_components(root, &staging_archive, "protected_archive_staging");
    if archive.exists() {
        fail(format!(
            "PROTECTED_HISTORY_IMMUTABLE:{}:{}",
            module_id, active_version
        ));
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
    publish_active_internal(root, module_id, version, false);
}

fn publish_active_rehydrated(root: &Path, module_id: &str, version: &str) {
    publish_active_internal(root, module_id, version, true);
}

fn publish_active_internal(
    root: &Path,
    module_id: &str,
    version: &str,
    historical_rehydrate: bool,
) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    assert_version(version, "INVALID_ACTIVE_VERSION");
    let project = read_project(root);
    assert_project_contract(root, &project);
    assert_declared_contracts(root, &project, true);
    assert_sdk_lock(root, &project);
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
    if let Some(version_base) = module.get("version_base").filter(|value| !value.is_null()) {
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
    if historical_rehydrate {
        assert_historical_frozen_record_graph(root, module_id, &artifact);
    } else {
        assert_record_graph(root, Some(module_id), &artifact, true);
    }
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
    if record.get("schema_version").and_then(Value::as_u64) != Some(1)
        || record.get("sdk").and_then(Value::as_str) != Some("appsdk")
        || record.get("version").and_then(Value::as_str) != Some("0.1.6")
    {
        fail("INVALID_SDK_RESOURCES");
    }
    for key in ["bundle_digest", "manifest_digest"] {
        let digest = record.get(key).and_then(Value::as_str).unwrap_or("");
        if digest.len() != 71
            || !digest.starts_with("sha256:")
            || !digest[7..].chars().all(|c| c.is_ascii_hexdigit())
        {
            fail("INVALID_SDK_RESOURCES_DIGEST");
        }
    }
    let entries = record
        .get("resources")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_SDK_RESOURCES"));
    for entry in entries {
        let source = entry
            .get("source")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fail("INVALID_SDK_RESOURCES"));
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
        {
            fail(format!("SDK_RESOURCE_PATH_ESCAPE:{}", relative));
        }
        let expected = entry
            .get("digest")
            .and_then(Value::as_str)
            .filter(|digest| {
                digest.len() == 71
                    && digest.starts_with("sha256:")
                    && digest[7..].chars().all(|c| c.is_ascii_hexdigit())
            })
            .unwrap_or_else(|| fail(format!("INVALID_SDK_RESOURCE_DIGEST:{}", source)));
        if entry.get("class").and_then(Value::as_str).is_none() {
            fail(format!("INVALID_SDK_RESOURCE_CLASS:{}", source));
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
        if let Some(version_base) = module.get("version_base").filter(|value| !value.is_null()) {
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
    assert_sdk_lock(root, &project);
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
                    assert_historical_frozen_record_graph(root, module_id, &module_artifact);
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

fn render_appsdk_gitignore(mut content: String) -> Result<String, String> {
    if let Some(begin) = content.find(APPSDK_GITIGNORE_BEGIN) {
        let end_start = begin + APPSDK_GITIGNORE_BEGIN.len();
        let end = content[end_start..]
            .find(APPSDK_GITIGNORE_END)
            .map(|offset| end_start + offset)
            .ok_or_else(|| "INVALID_APPSDK_GITIGNORE_BLOCK".to_string())?;
        let end_after = end + APPSDK_GITIGNORE_END.len();
        let mut updated = String::with_capacity(content.len());
        updated.push_str(&content[..begin]);
        updated.push_str(APPSDK_GITIGNORE_BLOCK);
        let suffix = &content[end_after..];
        if !suffix.trim().is_empty() {
            updated.push_str(suffix);
        }
        return Ok(updated);
    }
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.is_empty() {
        content.push('\n');
    }
    content.push_str(APPSDK_GITIGNORE_BLOCK);
    Ok(content)
}

fn ensure_appsdk_gitignore(root: &Path) {
    let path = root.join(".gitignore");
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:gitignore");
    }
    let content = fs::read_to_string(&path).unwrap_or_default();
    let updated = render_appsdk_gitignore(content.clone()).unwrap_or_else(|error| fail(error));
    if content != updated {
        fs::write(path, updated).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    }
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

fn write_project_agent_contract(root: &Path) {
    write_if_missing(root, "AGENTS.md", PROJECT_AGENTS_TEMPLATE);
}

fn write_current_sdk_lock(root: &Path) {
    let project = read_project(root);
    if project.pointer("/sdk/version").and_then(Value::as_str) != Some("0.1.6") {
        return;
    }
    let target = root.join(".appsdk/sdk.lock");
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_lock");
    }
    let existing = if target.exists() {
        let text = fs::read_to_string(&target).unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
        let value =
            serde_json::from_str::<Value>(&text).unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
        if value.get("sdk").and_then(Value::as_str) != Some("appsdk")
            || value.get("version").and_then(Value::as_str) != Some("0.1.6")
            || value.get("contract_schema") != project.get("schema_version")
        {
            fail("INVALID_SDK_LOCK");
        }
        Some(value)
    } else {
        None
    };
    let current_bundle_digest = sdk_bundle_digest();
    let mut lock = serde_json::Map::new();
    lock.insert("sdk".into(), Value::String("appsdk".into()));
    lock.insert("version".into(), Value::String("0.1.6".into()));
    lock.insert(
        "bundle_digest".into(),
        Value::String(current_bundle_digest.clone()),
    );
    lock.insert(
        "bundle_manifest_digest".into(),
        Value::String(digest_bytes(SDK_BUNDLE_MANIFEST.as_bytes())),
    );
    lock.insert("bundle_resources".into(), sdk_bundle_manifest_resources());
    lock.insert(
        "contract_schema".into(),
        project
            .get("schema_version")
            .cloned()
            .unwrap_or_else(|| fail("UNSUPPORTED_PROJECT_SCHEMA")),
    );
    if let Some(existing) = existing.as_ref() {
        for key in ["digest", "compiler_digest"] {
            if let Some(value) = existing.get(key).and_then(Value::as_str) {
                if value.len() == 71
                    && value.starts_with("sha256:")
                    && value[7..].chars().all(|c| c.is_ascii_hexdigit())
                {
                    lock.insert(key.into(), Value::String(value.into()));
                }
            }
        }
        if let Some(value) = existing.get("binary_ref").and_then(Value::as_str) {
            lock.insert("binary_ref".into(), Value::String(value.into()));
        }
        let existing_bundle = existing.get("bundle_digest").and_then(Value::as_str);
        let previous_bundle = if existing_bundle.is_some_and(|value| {
            value.len() == 71
                && value.starts_with("sha256:")
                && value[7..].chars().all(|c| c.is_ascii_hexdigit())
                && value != current_bundle_digest
        }) {
            existing_bundle
        } else {
            existing
                .get("previous_bundle_digest")
                .and_then(Value::as_str)
        };
        if let Some(value) = previous_bundle {
            if value.len() == 71
                && value.starts_with("sha256:")
                && value[7..].chars().all(|c| c.is_ascii_hexdigit())
            {
                lock.insert("previous_bundle_digest".into(), Value::String(value.into()));
            }
        }
    }
    atomic_write_json(&target, &Value::Object(lock), "SDK_LOCK_WRITE_FAILED");
}

fn install_standard_template_reference(root: &Path) {
    let target = root.join(".appsdk/templates/minimal/AGENTS.md");
    assert_no_symlink_components(root, &target, "guidance_standard_template");
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:guidance_standard_template");
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    }
    atomic_write_bytes(
        &target,
        PROJECT_AGENTS_TEMPLATE.as_bytes(),
        "GUIDANCE_STANDARD_TEMPLATE_WRITE_FAILED",
    );
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
  "guidance": {
    "enforcement": "advisory",
    "compiled_manifest": ".appsdk/guidance/compiled.json",
    "rule_sources": [
      {"source_id":"project-agents","kind":"agents","path":"AGENTS.md","required":false,"precedence":100},
      {"source_id":"appsdk-governance-skill","kind":"skill","path":".appsdk/skills/appsdk-project-governance/SKILL.md","contract_path":".appsdk/skills/appsdk-project-governance/appsdk-guidance.json","required":true,"precedence":200}
    ]
  },
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

fn existing_init_target(workspace: &Path, project_root: Option<&str>) -> Option<PathBuf> {
    let relative = project_root.unwrap_or(".");
    let relative_path = Path::new(relative);
    if relative.is_empty()
        || relative_path.is_absolute()
        || (relative != "."
            && relative_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            }))
    {
        fail("INVALID_PROJECT_ROOT");
    }
    let root = if relative == "." {
        workspace.to_path_buf()
    } else {
        workspace.join(relative_path)
    };
    if !root.join(".appsdk/project.json").is_file() {
        return None;
    }
    assert_no_symlink_components(workspace, &root, "existing_init_project");
    Some(root)
}

fn canonical_init_target(workspace: &Path, project_root: Option<&str>) -> PathBuf {
    let root = if let Some(project_root) = project_root {
        resolve_init_target(workspace, Some(project_root))
    } else {
        workspace.to_path_buf()
    };
    assert_init_workspace_safe(&root);
    if !root.exists() {
        return root.canonicalize().unwrap_or(root);
    }
    assert_no_symlink_components(workspace, &root, "existing_init_project");
    root.canonicalize()
        .unwrap_or_else(|_| fail(format!("PROJECT_ROOT_MISSING:{}", root.display())))
}

fn fresh_init_recovery_pending(root: &Path) -> bool {
    let transaction_dir = reset_transaction_dir(root);
    match fs::symlink_metadata(&transaction_dir) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => fail(format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:{}:{error}",
            transaction_dir.display()
        )),
    }
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

fn initialize_collab_peer() {
    if env::var_os("TMUX_PANE").is_none() {
        println!("collab peer bootstrap pending: no live tmux pane");
        println!(
            "collab-channel {}",
            serde_json::json!({
                "notification_channel":"none", "subscription_created":false,
                "independent_work_allowed":true,
                "next_action":"No push channel. Check subagent status (includes parent mailbox) yourself; use subagent snapshot explicitly for screen diagnostics. Do not wait for an automatic completion notification."
            })
        );
        return;
    }
    let output = match Command::new("collab").arg("init").output() {
        Ok(output) => output,
        Err(error) => {
            eprintln!("COLLAB_INIT_UNAVAILABLE:{}; shared collaboration unavailable; independent work may continue", error);
            return;
        }
    };
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        eprintln!("COLLAB_INIT_FAILED:{}; shared collaboration unavailable; independent work may continue", detail.trim());
        return;
    }
    let result = String::from_utf8_lossy(&output.stdout);
    if !result.trim().is_empty() {
        println!("collab {}", result.trim());
    }
}

fn init_project(root: &Path, fresh: bool, discard_legacy: bool) {
    if root.exists()
        && fs::symlink_metadata(root)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    {
        fail(format!("TARGET_SYMLINK:{}", root.display()));
    }
    if fresh {
        if !discard_legacy {
            fail("INIT_FRESH_REQUIRES_DISCARD_LEGACY_CONFIRMATION");
        }
        if !root.is_dir()
            || (!root.join(".appsdk/project.json").is_file() && !fresh_init_recovery_pending(root))
        {
            fail("INIT_FRESH_REQUIRES_EXISTING_PROJECT");
        }
    }
    fs::create_dir_all(root).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    if fresh {
        reset_governance_internal(root, true, true).unwrap_or_else(|error| fail(error));
        assert_fresh_project_contract_targets(root);
        initialize_collab_peer();
        if let Err(reason) = memory::initialize_project(root) {
            eprintln!("{}; optional project memory initialization skipped", reason);
        }
        println!("initialized fresh governance epoch {}", root.display());
        println!("next appsdk guide compile");
        println!(
            "then appsdk guide init --task <task-id> --mode <develop|debug> --module <module-id>"
        );
        return;
    }
    let fresh_governance = !root.join(".appsdk/project.json").is_file();
    let existing_project_needs_guidance = !fresh_governance
        && serde_json::from_str::<Value>(
            &fs::read_to_string(root.join(".appsdk/project.json"))
                .unwrap_or_else(|_| fail("INVALID_PROJECT")),
        )
        .unwrap_or_else(|_| fail("INVALID_PROJECT"))
        .get("guidance")
        .is_none();
    ensure_governance_layout(root);
    write_project_scaffold(root);
    if fresh_governance {
        write_project_agent_contract(root);
        install_bundle_resources(root);
    }
    write_current_sdk_lock(root);
    install_standard_template_reference(root);
    initialize_collab_peer();
    if let Err(reason) = memory::initialize_project(root) {
        eprintln!("{}; optional project memory initialization skipped", reason);
    }
    println!("initialized {}", root.display());
    if existing_project_needs_guidance {
        println!(
            "next appsdk guide init --task guidance-setup --mode bootstrap --module <module-id>"
        );
        println!("then read project documents and present GuidanceSetupProposal for user approval");
    } else if !fresh_governance {
        println!(
            "next appsdk guide init --task guidance-upgrade --mode bootstrap --module <module-id>"
        );
        println!(
            "then compare current project rules with the installed standard template and present a non-destructive GuidanceSetupProposal for user approval"
        );
    } else {
        println!("next appsdk guide compile");
        println!(
            "then appsdk guide init --task <task-id> --mode <develop|debug> --module <module-id>"
        );
    }
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
    write_project_agent_contract(root);
    install_bundle_resources(root);
    write_current_sdk_lock(root);
    install_standard_template_reference(root);
    if let Err(reason) = memory::initialize_project(root) {
        eprintln!("{}; optional project memory initialization skipped", reason);
    }
    println!("created {}", root.display());
    println!("next appsdk guide compile");
    println!("then appsdk guide init --task <task-id> --mode <develop|debug> --module <module-id>");
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

fn migration_bundle_transition_digest(root: &Path, record: &Value) -> Option<String> {
    let record_bundle = record
        .get("bundle_digest")
        .and_then(Value::as_str)
        .filter(|digest| digest.starts_with("sha256:"))?;
    let lock_path = root.join(".appsdk/sdk.lock");
    if !lock_path.is_file() {
        return None;
    }
    if fs::symlink_metadata(&lock_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_lock");
    }
    let lock: Value = serde_json::from_str(
        &fs::read_to_string(&lock_path).unwrap_or_else(|_| fail("INVALID_SDK_LOCK")),
    )
    .unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
    let lock_bundle = lock.get("bundle_digest").and_then(Value::as_str)?;
    let lock_previous_bundle = lock.get("previous_bundle_digest").and_then(Value::as_str);
    let current_bundle = sdk_bundle_digest();
    if record_bundle == current_bundle {
        return None;
    }
    let lock_bundle_is_valid = lock_bundle.len() == 71
        && lock_bundle.starts_with("sha256:")
        && lock_bundle[7..]
            .chars()
            .all(|byte| byte.is_ascii_hexdigit());
    if lock_bundle == record_bundle
        || (lock_bundle_is_valid && lock_previous_bundle == Some(record_bundle))
    {
        return Some(record_bundle.to_string());
    }
    None
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
        || record
            .get("bundle_digest")
            .and_then(Value::as_str)
            .filter(|digest| digest.starts_with("sha256:"))
            .is_none()
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
    let bundle_transition = migration_bundle_transition_digest(root, &record).is_some();
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
                    .is_some_and(|value| !value.is_null())
                // A witnessed upgrade preserves the historical canonical target.
                // Snapshot and live custom-map bytes remain checked below.
                && !(bundle_transition
                    && canonical_target.as_str().is_some_and(|digest| {
                        digest.strip_prefix("sha256:").is_some_and(|hex| {
                            hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                    })))
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
        let live_digest = file_sha256(&root.join(".appsdk/maps").join(name), "governance_map");
        let target_digest = record_str(entry, "/target_digest", "sdk-migration-map");
        let current_target = record_str(declared, "/target_digest", "sdk-map-migration");
        let current_target_is_authorized = bundle_transition
            && entry
                .get("canonical_source_digest")
                .is_none_or(Value::is_null)
            && entry
                .get("canonical_target_digest")
                .is_none_or(Value::is_null)
            && live_digest == current_target;
        if live_digest != target_digest && !current_target_is_authorized {
            fail(format!("SDK_MIGRATION_TARGET_MAP_MISMATCH:{}", name));
        }
    }
    let reviews = record
        .get("frozen_reviews")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
    let mut modules = std::collections::HashSet::new();
    let mut frozen_review_ids = std::collections::HashMap::new();
    for review in reviews {
        let module_id = record_str(review, "/module_id", "sdk-migration-review");
        assert_identifier(module_id, "INVALID_SDK_MIGRATION_RECORD");
        let review_id = record_str(review, "/review_id", "sdk-migration-review");
        if !modules.insert(module_id) || review_id.is_empty() {
            fail("INVALID_SDK_MIGRATION_RECORD");
        }
        frozen_review_ids.insert(module_id, review_id);
    }
    if let Some(legacy_reviews) = record.get("legacy_reconciled_reviews") {
        let legacy_reviews = legacy_reviews
            .as_array()
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        for review in legacy_reviews {
            let module_id = record_str(review, "/module_id", "sdk-migration-review");
            assert_identifier(module_id, "INVALID_SDK_MIGRATION_RECORD");
            let review_id = record_str(review, "/review_id", "sdk-migration-review");
            if review_id.is_empty()
                || record_str(review, "/stage", "sdk-migration-review") == "draft"
                || (modules.contains(module_id)
                    && frozen_review_ids.get(module_id) != Some(&review_id))
            {
                fail("INVALID_SDK_MIGRATION_RECORD");
            }
            modules.insert(module_id);
        }
    }
    Some(record)
}

fn install_current_governance_maps(root: &Path, force: bool) {
    let record_path = sdk_map_migration_root(root).join("record.json");
    if !force && record_path.is_file() {
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
        let record: Value = serde_json::from_str(
            &fs::read_to_string(migration_root.join("record.json"))
                .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD")),
        )
        .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD"));
        let manifest = sdk_map_migration_manifest();
        let source_maps = GOVERNANCE_MAP_NAMES.iter().all(|name| {
            file_sha256(&root.join(".appsdk/maps").join(name), "governance_map")
                == record_str(
                    sdk_map_migration_entry(&manifest, name),
                    "/source_digest",
                    "sdk-map-migration",
                )
        });
        let bundle_changed = record
            .get("bundle_digest")
            .and_then(Value::as_str)
            .is_some_and(|digest| digest != sdk_bundle_digest());
        let bundle_transition = migration_bundle_transition_digest(root, &record).is_some();
        if bundle_changed && !bundle_transition {
            fail("SDK_MIGRATION_BUNDLE_WITNESS_REQUIRED");
        }
        let has_custom_map_binding = record
            .pointer("/maps/0/canonical_source_digest")
            .is_some_and(Value::is_string);
        if !source_maps && !has_custom_map_binding {
            let current_maps = GOVERNANCE_MAP_NAMES.iter().all(|name| {
                file_sha256(&root.join(".appsdk/maps").join(name), "governance_map")
                    == record_str(
                        sdk_map_migration_entry(&manifest, name),
                        "/target_digest",
                        "sdk-map-migration",
                    )
            });
            let recorded_target_maps = GOVERNANCE_MAP_NAMES.iter().all(|name| {
                let entry = record
                    .get("maps")
                    .and_then(Value::as_array)
                    .and_then(|maps| {
                        maps.iter()
                            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
                    })
                    .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
                file_sha256(&root.join(".appsdk/maps").join(name), "governance_map")
                    == record_str(entry, "/target_digest", "sdk-migration-map")
            });
            if !current_maps && !recorded_target_maps {
                let detail = GOVERNANCE_MAP_NAMES
                    .iter()
                    .find(|name| {
                        let live =
                            file_sha256(&root.join(".appsdk/maps").join(name), "governance_map");
                        let current_target = record_str(
                            sdk_map_migration_entry(&manifest, name),
                            "/target_digest",
                            "sdk-map-migration",
                        );
                        let entry = record
                            .get("maps")
                            .and_then(Value::as_array)
                            .and_then(|maps| {
                                maps.iter().find(|entry| {
                                    entry.get("name").and_then(Value::as_str) == Some(name)
                                })
                            })
                            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
                        live != current_target
                            && live != record_str(entry, "/target_digest", "sdk-migration-map")
                    })
                    .copied()
                    .unwrap_or("mixed");
                fail(format!("SDK_MIGRATION_LIVE_MAP_UNRECONCILED:{detail}"));
            }
        }
        if source_maps {
            for module in project
                .get("modules")
                .and_then(Value::as_array)
                .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"))
            {
                let module_id = record_str(module, "/module_id", "module");
                let stage = module
                    .get("stage")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| fail(format!("INVALID_MODULE_CONTRACT:{}", module_id)));
                if !matches!(stage, "frozen" | "retired") {
                    continue;
                }
                let review_name = module_record_name("review-record", module_id);
                let current_review = read_record(root, &review_name);
                for name in GOVERNANCE_MAP_NAMES {
                    let entry = sdk_map_migration_entry(&manifest, name);
                    let field = record_str(entry, "/review_hash_field", "sdk-map-migration");
                    if record_str(&current_review, &format!("/{}", field), &review_name)
                        != record_str(entry, "/source_digest", "sdk-map-migration")
                    {
                        fail(format!(
                            "SDK_MIGRATION_FROZEN_REVIEW_MAP_MISMATCH:{}:{}",
                            module_id, name
                        ));
                    }
                }
            }
        }
        install_current_governance_maps(root, source_maps);
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
            if stage == "draft" || review.get("verdict").and_then(Value::as_str) != Some("pass") {
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
        if matches!(stage, "frozen" | "retired") {
            frozen_reviews.push(serde_json::json!({
                "module_id": module_id,
                "review_id": record_str(&review, "/review_id", &review_name)
            }));
        }
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
    install_current_governance_maps(root, false);
    let _ = assert_sdk_migration_record(root);
    assert_governance_maps(root);
}

fn write_legacy_migration_step(root: &Path, source_version: &str) {
    let migration_root = root
        .join(".appsdk")
        .join("migrations")
        .join(format!("{}-to-0.1.5", source_version));
    let record = migration_root.join("record.json");
    if record.is_file() {
        return;
    }
    if migration_root.exists() {
        fail("SDK_LEGACY_MIGRATION_RECORD_MISSING");
    }
    let staging = root
        .join(".appsdk")
        .join("migrations")
        .join(format!(".{}-to-0.1.5.staging", source_version));
    if staging.exists() {
        fail("SDK_LEGACY_MIGRATION_STAGING_EXISTS");
    }
    fs::create_dir_all(staging.join("maps"))
        .unwrap_or_else(|_| fail("SDK_LEGACY_MIGRATION_WRITE_FAILED"));
    let mut maps = Vec::new();
    for name in GOVERNANCE_MAP_NAMES {
        let source = root.join(".appsdk/maps").join(name);
        if !source.is_file() {
            fail(format!("MISSING_GOVERNANCE_MAP:{}", name));
        }
        atomic_write_bytes(
            &staging.join("maps").join(name),
            &fs::read(&source).unwrap_or_else(|_| fail("SDK_LEGACY_MIGRATION_READ_FAILED")),
            "SDK_LEGACY_MIGRATION_WRITE_FAILED",
        );
        let digest = file_sha256(&source, "legacy_governance_map");
        maps.push(serde_json::json!({
            "name": name,
            "source_digest": digest,
            "target_digest": digest,
            "snapshot_path": format!(".appsdk/migrations/{}-to-0.1.5/maps/{}", source_version, name)
        }));
    }
    atomic_write_json(
        &staging.join("record.json"),
        &serde_json::json!({
            "schema_version": 1,
            "migration_id": format!("appsdk-{}-to-0.1.5", source_version),
            "source_version": source_version,
            "target_version": "0.1.5",
            "maps": maps,
            "preserved_project_maps": true,
            "created_at": Utc::now().to_rfc3339()
        }),
        "SDK_LEGACY_MIGRATION_WRITE_FAILED",
    );
    fs::rename(&staging, &migration_root)
        .unwrap_or_else(|_| fail("SDK_LEGACY_MIGRATION_WRITE_FAILED"));
}

fn install_current_project_contract(
    root: &Path,
    relative: &str,
    canonical: &str,
    replace_legacy: bool,
) {
    let target = root.join(relative);
    assert_no_symlink_components(root, &target, "governance_contract_migration");
    let canonical: Value = serde_json::from_str(canonical)
        .unwrap_or_else(|_| fail("INVALID_CANONICAL_RECORD_CONTRACT"));
    if !target.is_file() {
        return;
    }
    if !replace_legacy {
        let current: Value = serde_json::from_str(
            &fs::read_to_string(&target)
                .unwrap_or_else(|_| fail("SDK_RECORD_CONTRACT_MIGRATION_READ_FAILED")),
        )
        .unwrap_or_else(|_| fail("SDK_RECORD_CONTRACT_MIGRATION_READ_FAILED"));
        if current == canonical {
            return;
        }
    }
    let mut content = serde_json::to_vec_pretty(&canonical)
        .unwrap_or_else(|_| fail("SDK_RECORD_CONTRACT_MIGRATION_WRITE_FAILED"));
    content.push(b'\n');
    atomic_write_bytes(
        &target,
        &content,
        "SDK_RECORD_CONTRACT_MIGRATION_WRITE_FAILED",
    );
}

fn install_current_project_contracts(root: &Path, prefixes: &[&str], replace_legacy: bool) {
    for &(relative, _, canonical) in SDK_BUNDLE_RESOURCES
        .iter()
        .filter(|(path, _, _)| prefixes.iter().any(|prefix| path.starts_with(prefix)))
    {
        install_current_project_contract(root, relative, canonical, replace_legacy);
    }
}

fn install_current_record_contracts(root: &Path) {
    install_current_project_contracts(root, &["contracts/records/"], false);
}

fn assert_fresh_project_contract_target(root: &Path, relative: &str) {
    let target = root.join(relative);
    assert_no_symlink_components(root, &target, "governance_contract_migration");
    match fs::symlink_metadata(&target) {
        Ok(metadata) if !metadata.is_file() => {
            fail(format!("GOVERNANCE_CONTRACT_NOT_FILE:{}", relative));
        }
        Ok(_) => {}
        Err(error) if error.kind() != ErrorKind::NotFound => {
            fail(format!("GOVERNANCE_CONTRACT_METADATA_FAILED:{}", relative));
        }
        Err(_) => {}
    }
}

fn assert_fresh_project_contract_targets(root: &Path) {
    for &(relative, _, _) in SDK_BUNDLE_RESOURCES.iter().filter(|(path, _, _)| {
        path.starts_with("contracts/records/") || path.starts_with("contracts/transitions/")
    }) {
        assert_fresh_project_contract_target(root, relative);
    }
    assert_fresh_project_contract_target(
        root,
        "contracts/transitions/zone-transition-manifest.json",
    );
}

#[derive(Clone)]
struct ResetTransactionTarget {
    relative: String,
    original: PathBuf,
    backup: PathBuf,
    staged: Option<PathBuf>,
    kind: &'static str,
    original_exists: bool,
    quarantined: bool,
    published: bool,
}

#[cfg(unix)]
const RESET_TRANSACTION_LOCK_EX: c_int = 2;
#[cfg(unix)]
const RESET_TRANSACTION_LOCK_NB: c_int = 4;

#[cfg(unix)]
unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
}

fn reset_transaction_symlink_components(base: &Path, path: &Path) -> Result<(), String> {
    if fs::symlink_metadata(base)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:symlink base {}",
            base.display()
        ));
    }
    let relative = path
        .strip_prefix(base)
        .map_err(|_| "GOVERNANCE_RESET_RECOVERY_REQUIRED:path escape".to_string())?;
    let mut current = base.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        if fs::symlink_metadata(&current)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:symlink component {}",
                current.display()
            ));
        }
    }
    Ok(())
}

fn reset_transaction_lock_path(root: &Path) -> PathBuf {
    let transaction_dir = reset_transaction_dir(root);
    PathBuf::from(format!("{}.lock", transaction_dir.display()))
}

struct ResetTransactionLock {
    _file: fs::File,
    path: PathBuf,
}

impl Drop for ResetTransactionLock {
    fn drop(&mut self) {
        // Keep the advisory lock held while removing its pathname. A new
        // transaction can only create and lock a replacement inode after the
        // path is gone, so cleanup cannot remove that replacement.
        let _ = fs::remove_file(&self.path);
    }
}

fn reset_transaction_acquire_lock(root: &Path) -> Result<ResetTransactionLock, String> {
    let lock_path = reset_transaction_lock_path(root);
    if fs::symlink_metadata(&lock_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "GOVERNANCE_RESET_LOCK_SYMLINK:{}",
            lock_path.display()
        ));
    }
    if let Some(parent) = lock_path.parent() {
        reset_transaction_symlink_components(parent, &lock_path)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| format!("GOVERNANCE_RESET_LOCK_FAILED:{error}"))?;
    #[cfg(unix)]
    {
        if unsafe {
            flock(
                file.as_raw_fd(),
                RESET_TRANSACTION_LOCK_EX | RESET_TRANSACTION_LOCK_NB,
            )
        } != 0
        {
            return Err(format!("GOVERNANCE_RESET_BUSY:{}", lock_path.display()));
        }
    }
    Ok(ResetTransactionLock {
        _file: file,
        path: lock_path,
    })
}

fn reset_transaction_expected_target_kind(
    relative: &str,
    generated_roots: &[String],
) -> Option<&'static str> {
    if relative == ".appsdk" || relative == ".appsdk-control" {
        Some("dir")
    } else if reset_transaction_quarantine_generated_roots(generated_roots)
        .iter()
        .any(|root| root == relative)
    {
        Some("dir")
    } else if reset_transaction_fresh_project_targets()
        .iter()
        .any(|target| target == relative)
        || relative == ".gitignore"
    {
        Some("file")
    } else {
        None
    }
}

fn reset_transaction_allowed_target_relative(relative: &str, generated_roots: &[String]) -> bool {
    reset_transaction_expected_target_kind(relative, generated_roots).is_some()
}

fn reset_transaction_contract_target_relative(relative: &str) -> bool {
    relative.starts_with("contracts/records/") || relative.starts_with("contracts/transitions/")
}

fn reset_transaction_expected_staged_relative(relative: &str) -> Option<String> {
    if relative == ".appsdk" || relative == ".appsdk-control" || relative == ".gitignore" {
        Some(format!("staging/{relative}"))
    } else if reset_transaction_contract_target_relative(relative) || relative == "generated" {
        Some(format!("staging/{relative}"))
    } else {
        None
    }
}

fn reset_transaction_allowed_created_dir_relative(relative: &str) -> bool {
    matches!(
        relative,
        "contracts" | "contracts/records" | "contracts/transitions"
    )
}

fn reset_transaction_parse_generated_root(
    relative: &str,
    case_insensitive: bool,
) -> Result<String, String> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err("INVALID_GOVERNANCE_ROOT:/governance/generated_root".into());
    }
    if reset_root_conflicts_with_reserved(relative, case_insensitive) {
        return Err("RESET_GENERATED_ROOT_CONFLICT".into());
    }
    Ok(relative.to_string())
}

fn reset_transaction_parse_generated_roots(
    project: &Value,
    case_insensitive: bool,
) -> Result<Vec<String>, String> {
    let declared = project
        .pointer("/governance/generated_root")
        .and_then(Value::as_str)
        .ok_or_else(|| "INVALID_GOVERNANCE_ROOT:/governance/generated_root".to_string())?;
    let relative = declared.trim_end_matches("/**").trim_end_matches('/');
    let relative = reset_transaction_parse_generated_root(relative, case_insensitive)?;
    let mut roots = vec!["generated".to_string()];
    if !roots.iter().any(|existing| existing == &relative) {
        roots.push(relative);
    }
    Ok(roots)
}

fn reset_transaction_generated_root_allowed(relative: &str) -> bool {
    reset_transaction_validate_relative(relative).is_ok()
        && reset_transaction_parse_generated_root(relative, true).is_ok()
}

fn reset_transaction_created_dirs(
    root: &Path,
    targets: &[ResetTransactionTarget],
) -> Result<Vec<String>, String> {
    let mut created = BTreeSet::new();
    for target in targets {
        if target.kind != "file" || !reset_transaction_contract_target_relative(&target.relative) {
            continue;
        }
        let mut parent = Path::new(&target.relative).parent();
        while let Some(dir) = parent {
            let relative = dir.to_string_lossy().replace('\\', "/");
            if reset_transaction_allowed_created_dir_relative(&relative)
                && fs::symlink_metadata(root.join(&relative)).is_err()
            {
                created.insert(relative);
            }
            parent = dir.parent();
        }
    }
    let mut dirs = created.into_iter().collect::<Vec<_>>();
    dirs.sort_by_key(|relative| relative.matches('/').count());
    Ok(dirs)
}

fn reset_transaction_read_project_contract(path: &Path) -> Result<Value, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:project contract unavailable:{}:{error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid project contract {}",
            path.display()
        ));
    }
    let text = fs::read_to_string(path).map_err(|error| {
        format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:project contract unavailable:{}:{error}",
            path.display()
        )
    })?;
    serde_json::from_str(&text).map_err(|error| {
        format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid project contract {}:{error}",
            path.display()
        )
    })
}

fn reset_transaction_recovery_project_contract(
    root: &Path,
    transaction_dir: &Path,
    marker: &Value,
) -> Result<Value, String> {
    let values = marker
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing targets".to_string())?;

    // During publishing the replacement `.appsdk` may already be visible at
    // the project root while the old contract is still in quarantine. The
    // old contract owns the generated-root deletion plan for this transaction;
    // prefer it whenever the marker proves that quarantine binding.
    for (index, value) in values.iter().enumerate() {
        if value.get("relative").and_then(Value::as_str) != Some(".appsdk")
            || value.get("kind").and_then(Value::as_str) != Some("dir")
            || value.get("original_exists").and_then(Value::as_bool) != Some(true)
        {
            continue;
        }
        let backup = value.get("backup").and_then(Value::as_str).ok_or_else(|| {
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing appsdk backup".to_string()
        })?;
        let expected = format!("quarantine/target-{index}");
        if backup != expected {
            return Err(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid appsdk quarantine binding".into(),
            );
        }
        reset_transaction_validate_relative(backup)?;
        let backup_root = transaction_dir.join(backup);
        reset_transaction_symlink_components(transaction_dir, &backup_root)?;
        let metadata = match fs::symlink_metadata(&backup_root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "GOVERNANCE_RESET_RECOVERY_REQUIRED:appsdk quarantine unavailable:{}:{error}",
                    backup_root.display()
                ))
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid appsdk quarantine binding".into(),
            );
        }
        return reset_transaction_read_project_contract(&backup_root.join("project.json"));
    }

    let project = project_file(root);
    match fs::symlink_metadata(&project) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err("GOVERNANCE_RESET_RECOVERY_REQUIRED:project contract symlink".into())
        }
        Ok(metadata) if metadata.is_file() => {
            return reset_transaction_read_project_contract(&project)
        }
        Ok(_) => {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid project contract {}",
                project.display()
            ))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:project contract unavailable:{}:{error}",
                project.display()
            ))
        }
    }
    Err("GOVERNANCE_RESET_RECOVERY_REQUIRED:project contract unavailable".into())
}

fn reset_transaction_recovery_generated_roots(
    root: &Path,
    transaction_dir: &Path,
    marker: &Value,
    phase: &str,
) -> Result<Vec<String>, String> {
    let marker_roots = reset_transaction_marker_generated_roots(marker)?;
    let project = reset_transaction_recovery_project_contract(root, transaction_dir, marker);
    let project_path = project_file(root);
    let derived = project.and_then(|project| {
        let case_insensitive = if project_path.is_file() {
            reset_root_filesystem_is_case_insensitive(root)
        } else {
            // The old `.appsdk` may already be quarantined. Rejecting case
            // variants conservatively keeps recovery from treating an alias
            // as a new root.
            true
        };
        reset_transaction_parse_generated_roots(&project, case_insensitive)
    });

    match (derived, marker_roots) {
        (Ok(roots), Some(marker_roots)) => {
            if roots.iter().cloned().collect::<BTreeSet<_>>() == marker_roots {
                for relative in &roots {
                    reset_transaction_symlink_components(root, &root.join(relative))?;
                }
                return Ok(roots);
            }
            if matches!(phase, "committed" | "cleanup_failed")
                && reset_transaction_committed_record_matches(root, marker)
            {
                return Ok(marker_roots.into_iter().collect());
            }
            Err("GOVERNANCE_RESET_RECOVERY_REQUIRED:generated root binding mismatch".into())
        }
        (Ok(roots), None) => {
            for relative in &roots {
                reset_transaction_symlink_components(root, &root.join(relative))?;
            }
            Ok(roots)
        }
        (Err(_error), Some(marker_roots))
            if matches!(phase, "committed" | "cleanup_failed")
                && reset_transaction_committed_record_matches(root, marker) =>
        {
            Ok(marker_roots.into_iter().collect())
        }
        (Err(error), _) => Err(error),
    }
}

fn reset_transaction_marker_generated_roots(
    marker: &Value,
) -> Result<Option<BTreeSet<String>>, String> {
    let Some(values) = marker.get("generated_roots") else {
        return Ok(None);
    };
    let values = values
        .as_array()
        .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid generated_roots".to_string())?;
    let mut roots = BTreeSet::new();
    for value in values {
        let relative = value.as_str().ok_or_else(|| {
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid generated_roots".to_string()
        })?;
        reset_transaction_validate_relative(relative)?;
        if !reset_transaction_generated_root_allowed(relative) {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:illegal generated root {relative}"
            ));
        }
        if !roots.insert(relative.to_string()) {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:duplicate generated root {relative}"
            ));
        }
    }
    if !roots.contains("generated") {
        return Err("GOVERNANCE_RESET_RECOVERY_REQUIRED:missing generated root".into());
    }
    Ok(Some(roots))
}

fn reset_transaction_committed_record_matches(root: &Path, marker: &Value) -> bool {
    let transaction_id = marker
        .get("transaction_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let Some(transaction_id) = transaction_id else {
        return false;
    };
    let path = root
        .join(".appsdk")
        .join("records")
        .join("reset-governance-record.json");
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return false;
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return false;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(record) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    record.get("mode").and_then(Value::as_str) == Some("fresh_init")
        && record.get("transaction_id").and_then(Value::as_str) == Some(transaction_id)
        && record.get("reset_id").and_then(Value::as_str) == Some(transaction_id)
}

fn reset_transaction_marker_root_matches(
    root: &Path,
    transaction_dir: &Path,
    marker_root: &str,
) -> bool {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if marker_root == root.to_string_lossy().as_ref()
        || marker_root == canonical_root.to_string_lossy().as_ref()
    {
        return true;
    }
    let marker_path = Path::new(marker_root);
    let mut candidates = Vec::new();
    if marker_path.is_absolute() {
        candidates.push(marker_path.to_path_buf());
    } else {
        candidates.push(transaction_dir.parent().unwrap_or(root).join(marker_path));
        if let Ok(current) = env::current_dir() {
            candidates.push(current.join(marker_path));
        }
    }
    candidates
        .into_iter()
        .filter_map(|candidate| candidate.canonicalize().ok())
        .any(|candidate| candidate == canonical_root)
}

fn reset_transaction_validate_marker(
    root: &Path,
    transaction_dir: &Path,
    marker: &Value,
) -> Result<(), String> {
    if marker.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid schema_version".into());
    }
    let marker_root = marker
        .get("root")
        .and_then(Value::as_str)
        .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing root".to_string())?;
    if !reset_transaction_marker_root_matches(root, transaction_dir, marker_root) {
        return Err("GOVERNANCE_RESET_RECOVERY_REQUIRED:root mismatch".into());
    }
    let transaction_id = marker
        .get("transaction_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing transaction_id".to_string())?;
    let _ = transaction_id;
    if !marker.get("error").map_or(true, Value::is_null)
        && !marker.get("error").map_or(false, Value::is_string)
    {
        return Err("GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid error".into());
    }
    let phase = marker
        .get("phase")
        .and_then(Value::as_str)
        .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing phase".to_string())?;
    if !matches!(
        phase,
        "building"
            | "build_failed"
            | "preflight_failed"
            | "prepared"
            | "quarantining"
            | "publishing"
            | "committed"
            | "cleanup_failed"
            | "rollback_failed"
    ) {
        return Err(format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid phase {phase}"
        ));
    }
    let generated_roots =
        reset_transaction_recovery_generated_roots(root, transaction_dir, marker, phase)?;
    let created_dirs = marker
        .get("created_dirs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut seen_created = BTreeSet::new();
    for created in &created_dirs {
        let relative = created
            .as_str()
            .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid created_dirs".to_string())?;
        reset_transaction_validate_relative(relative)?;
        if !reset_transaction_allowed_created_dir_relative(relative) {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:illegal created_dirs {relative}"
            ));
        }
        if !seen_created.insert(relative.to_string()) {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:duplicate created_dirs {relative}"
            ));
        }
        reset_transaction_symlink_components(root, &root.join(relative))?;
    }
    let values = marker
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing targets".to_string())?;
    if matches!(phase, "building" | "build_failed" | "preflight_failed")
        && (!values.is_empty() || !created_dirs.is_empty())
    {
        return Err("GOVERNANCE_RESET_RECOVERY_REQUIRED:early phase targets present".into());
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        let relative = value
            .get("relative")
            .and_then(Value::as_str)
            .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing target".to_string())?;
        reset_transaction_validate_relative(relative)?;
        if !reset_transaction_allowed_target_relative(relative, &generated_roots) {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:illegal target {relative}"
            ));
        }
        let expected_kind =
            reset_transaction_expected_target_kind(relative, &generated_roots).unwrap_or("file");
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing target kind".to_string())?;
        if kind != expected_kind {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid target kind {relative}"
            ));
        }
        let original_exists = value
            .get("original_exists")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                format!("GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid original_exists for {relative}")
            })?;
        let quarantined = value
            .get("quarantined")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                format!("GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid quarantined for {relative}")
            })?;
        let published = value
            .get("published")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                format!("GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid published for {relative}")
            })?;
        let backup_rel = value
            .get("backup")
            .and_then(Value::as_str)
            .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing backup".to_string())?;
        reset_transaction_validate_relative(backup_rel)?;
        let expected_backup = format!("quarantine/target-{index}");
        if backup_rel != expected_backup {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid backup {relative}"
            ));
        }
        let backup = transaction_dir.join(backup_rel);
        reset_transaction_symlink_components(transaction_dir, &backup)?;
        let backup_exists = match fs::symlink_metadata(&backup) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(format!(
                        "GOVERNANCE_RESET_RECOVERY_REQUIRED:symlink backup {relative}"
                    ));
                }
                if (expected_kind == "dir" && !metadata.is_dir())
                    || (expected_kind == "file" && !metadata.is_file())
                {
                    return Err(format!(
                        "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid backup kind {relative}"
                    ));
                }
                true
            }
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => {
                return Err(format!(
                    "GOVERNANCE_RESET_RECOVERY_REQUIRED:backup metadata {relative}:{error}"
                ))
            }
        };
        let staged = value.get("staged").cloned();
        let staged_relative = match staged {
            Some(Value::Null) => None,
            Some(Value::String(value)) => Some(value),
            _ => {
                return Err(format!(
                    "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid staged {relative}"
                ))
            }
        };
        let expected_staged = reset_transaction_expected_staged_relative(relative);
        if staged_relative != expected_staged {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid staged binding {relative}"
            ));
        }
        let original = root.join(relative);
        reset_transaction_symlink_components(root, &original)?;
        let original_present = match fs::symlink_metadata(&original) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(format!(
                        "GOVERNANCE_RESET_RECOVERY_REQUIRED:symlink original {relative}"
                    ));
                }
                if (expected_kind == "dir" && !metadata.is_dir())
                    || (expected_kind == "file" && !metadata.is_file())
                {
                    return Err(format!(
                        "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid original kind {relative}"
                    ));
                }
                true
            }
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => {
                return Err(format!(
                    "GOVERNANCE_RESET_RECOVERY_REQUIRED:original metadata {relative}:{error}"
                ))
            }
        };
        let staged_present = if let Some(staged_relative) = staged_relative.as_ref() {
            let staged = transaction_dir.join(staged_relative);
            match fs::symlink_metadata(&staged) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() {
                        return Err(format!(
                            "GOVERNANCE_RESET_RECOVERY_REQUIRED:symlink staged {relative}"
                        ));
                    }
                    if (expected_kind == "dir" && !metadata.is_dir())
                        || (expected_kind == "file" && !metadata.is_file())
                    {
                        return Err(format!(
                            "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid staged kind {relative}"
                        ));
                    }
                    true
                }
                Err(error) if error.kind() == ErrorKind::NotFound => false,
                Err(error) => {
                    return Err(format!(
                        "GOVERNANCE_RESET_RECOVERY_REQUIRED:staged metadata {relative}:{error}"
                    ))
                }
            }
        } else {
            false
        };
        let quarantine_marker_lag =
            original_exists && !original_present && !quarantined && backup_exists;
        let rollback_marker_lag =
            original_exists && original_present && quarantined && !backup_exists;
        if !original_exists && (quarantined || backup_exists) {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:absent target has quarantine state {relative}"
            ));
        }
        if original_exists
            && !original_present
            && !backup_exists
            && !(matches!(phase, "committed" | "cleanup_failed")
                && !published
                && staged_relative.is_none())
        {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing original state {relative}"
            ));
        }
        if original_exists && original_present && backup_exists && !quarantined {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:ambiguous original and backup state {relative}"
            ));
        }
        if !original_exists && original_present && staged_present {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:absent target has original and staging {relative}"
            ));
        }
        if !original_exists
            && original_present
            && !published
            && !staged_present
            && !matches!(phase, "quarantining" | "publishing")
        {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:unexpected original state {relative}"
            ));
        }
        if original_exists && published && !original_present && !backup_exists {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:published target missing original {relative}"
            ));
        }
        if published && staged_relative.is_none() {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:published target missing staging {relative}"
            ));
        }
        if original_exists && published && !quarantined {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:published target not quarantined {relative}"
            ));
        }
        if !matches!(phase, "committed" | "cleanup_failed")
            && quarantined != backup_exists
            && !quarantine_marker_lag
            && !rollback_marker_lag
        {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:backup state mismatch {relative}"
            ));
        }
        if matches!(phase, "building" | "build_failed" | "preflight_failed")
            && (quarantined || published)
        {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:early phase state mismatch {relative}"
            ));
        }
        if phase == "prepared" && (published || (quarantined && !rollback_marker_lag)) {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:prepared phase state mismatch {relative}"
            ));
        }
        if phase == "quarantining" && published {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:quarantining phase state mismatch {relative}"
            ));
        }
        if matches!(phase, "committed" | "cleanup_failed")
            && ((staged_relative.is_some() && !published)
                || (staged_relative.is_none() && published))
        {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:committed phase state mismatch {relative}"
            ));
        }
        if let Some(staged_relative) = staged_relative {
            reset_transaction_validate_relative(&staged_relative)?;
            reset_transaction_symlink_components(
                transaction_dir,
                &transaction_dir.join(&staged_relative),
            )?;
        }
        if !seen.insert(relative.to_string()) {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:duplicate target {relative}"
            ));
        }
    }
    let relatives = seen.iter().cloned().collect::<Vec<_>>();
    for i in 0..relatives.len() {
        for j in (i + 1)..relatives.len() {
            let parent = &relatives[i];
            let child = &relatives[j];
            if parent == child
                || child.starts_with(&format!("{parent}/"))
                || parent.starts_with(&format!("{child}/"))
            {
                return Err(format!(
                    "GOVERNANCE_RESET_RECOVERY_REQUIRED:overlapping targets {parent} {child}"
                ));
            }
        }
    }
    reset_transaction_validate_quarantine_entries(&transaction_dir, values)?;
    let early_phase = matches!(phase, "building" | "build_failed" | "preflight_failed");
    let plan_phase = matches!(
        phase,
        "prepared"
            | "quarantining"
            | "publishing"
            | "committed"
            | "cleanup_failed"
            | "rollback_failed"
    );
    if early_phase {
        if !reset_transaction_directory_empty(&transaction_dir.join("quarantine"))? {
            return Err(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:early phase quarantine is not empty".into(),
            );
        }
    } else if plan_phase {
        if values.is_empty() {
            if !reset_transaction_directory_empty(&transaction_dir.join("staging"))? {
                return Err(
                    "GOVERNANCE_RESET_RECOVERY_REQUIRED:incomplete transaction plan".into(),
                );
            }
        } else if seen != reset_transaction_expected_target_relatives(&generated_roots) {
            return Err("GOVERNANCE_RESET_RECOVERY_REQUIRED:incomplete transaction plan".into());
        }
    }
    if matches!(phase, "committed" | "cleanup_failed")
        && !reset_transaction_committed_record_matches(root, marker)
    {
        return Err("GOVERNANCE_RESET_RECOVERY_REQUIRED:missing committed reset record".into());
    }
    Ok(())
}

fn reset_transaction_dir(root: &Path) -> PathBuf {
    let name = root
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("project");
    root.parent()
        .unwrap_or(root)
        .join(format!(".appsdk-reset-transaction-{name}"))
}

fn reset_transaction_id() -> Result<String, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("RESET_TRANSACTION_NONCE_FAILED:{error}"))?
        .as_nanos();
    Ok(format!("fresh-init-{}-{nonce}", std::process::id()))
}

fn reset_transaction_write_bytes(
    transaction_dir: &Path,
    target: &Path,
    bytes: &[u8],
) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| "RESET_TRANSACTION_TARGET_INVALID".to_string())?;
    reset_transaction_symlink_components(transaction_dir, parent)?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("RESET_TRANSACTION_PARENT_CREATE_FAILED:{error}"))?;
    if fs::symlink_metadata(target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "RESET_TRANSACTION_TARGET_SYMLINK:{}",
            target.display()
        ));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("RESET_TRANSACTION_NONCE_FAILED:{error}"))?
        .as_nanos();
    let staging = target.with_extension(format!("staging.{}.{}", std::process::id(), nonce));
    fs::write(&staging, bytes).map_err(|error| {
        format!(
            "RESET_TRANSACTION_WRITE_FAILED:{}:{error}",
            target.display()
        )
    })?;
    if let Err(error) = fs::rename(&staging, target) {
        let _ = fs::remove_file(&staging);
        return Err(format!(
            "RESET_TRANSACTION_PUBLISH_FAILED:{}:{error}",
            target.display()
        ));
    }
    Ok(())
}

fn reset_transaction_is_marker_temp(name: &str) -> bool {
    let Some(suffix) = name.strip_prefix("marker.staging.") else {
        return false;
    };
    let mut parts = suffix.split('.');
    let (Some(pid), Some(nonce), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !pid.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_digit())
        && !nonce.is_empty()
        && nonce.bytes().all(|byte| byte.is_ascii_digit())
}

fn reset_transaction_write_json(
    transaction_dir: &Path,
    target: &Path,
    value: &Value,
) -> Result<(), String> {
    let content = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("RESET_TRANSACTION_JSON_FAILED:{error}"))?;
    let mut bytes = content;
    bytes.push(b'\n');
    reset_transaction_write_bytes(transaction_dir, target, &bytes)
}

fn reset_transaction_remove_path(base: &Path, path: &Path) -> Result<(), String> {
    reset_transaction_symlink_components(base, path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "RESET_TRANSACTION_METADATA_FAILED:{}:{error}",
                path.display()
            ))
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(format!("RESET_TRANSACTION_PATH_SYMLINK:{}", path.display()));
    }
    if metadata.is_dir() {
        fs::remove_dir_all(path)
            .map_err(|error| format!("RESET_TRANSACTION_REMOVE_FAILED:{}:{error}", path.display()))
    } else {
        fs::remove_file(path)
            .map_err(|error| format!("RESET_TRANSACTION_REMOVE_FAILED:{}:{error}", path.display()))
    }
}

fn reset_transaction_directory_empty(path: &Path) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(true),
        Err(error) => {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:directory metadata {}:{error}",
                path.display()
            ))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid transaction directory {}",
            path.display()
        ));
    }
    let mut entries = fs::read_dir(path).map_err(|error| {
        format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:directory read {}:{error}",
            path.display()
        )
    })?;
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(|error| {
            format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:directory read {}:{error}",
                path.display()
            )
        })?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
            format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:directory entry {}:{error}",
                entry.path().display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:symlink transaction entry {}",
                entry.path().display()
            ));
        }
        return Ok(false);
    }
    Ok(true)
}

fn reset_transaction_validate_quarantine_entries(
    transaction_dir: &Path,
    values: &[Value],
) -> Result<(), String> {
    let quarantine = transaction_dir.join("quarantine");
    reset_transaction_symlink_components(transaction_dir, &quarantine)?;
    let metadata = match fs::symlink_metadata(&quarantine) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:quarantine metadata {}:{error}",
                quarantine.display()
            ))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid quarantine directory {}",
            quarantine.display()
        ));
    }
    let expected = values
        .iter()
        .enumerate()
        .map(|(index, _)| format!("target-{index}"))
        .collect::<BTreeSet<_>>();
    let mut entries = fs::read_dir(&quarantine).map_err(|error| {
        format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:quarantine read {}:{error}",
            quarantine.display()
        )
    })?;
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(|error| {
            format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:quarantine read {}:{error}",
                quarantine.display()
            )
        })?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid quarantine entry {}",
                entry.path().display()
            )
        })?;
        let entry_metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
            format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:quarantine entry {}:{error}",
                entry.path().display()
            )
        })?;
        if entry_metadata.file_type().is_symlink() {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:symlink quarantine entry {}",
                entry.path().display()
            ));
        }
        if !expected.contains(name) {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:unexpected quarantine entry {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn reset_transaction_validate_cleanup_layout(transaction_dir: &Path) -> Result<(), String> {
    let mut entries = fs::read_dir(transaction_dir).map_err(|error| {
        format!(
            "GOVERNANCE_RESET_CLEANUP_FAILED:transaction read {}:{error}",
            transaction_dir.display()
        )
    })?;
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(|error| {
            format!(
                "GOVERNANCE_RESET_CLEANUP_FAILED:transaction read {}:{error}",
                transaction_dir.display()
            )
        })?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            format!(
                "GOVERNANCE_RESET_CLEANUP_FAILED:unexpected transaction entry {}",
                entry.path().display()
            )
        })?;
        let marker_temp = reset_transaction_is_marker_temp(name);
        if !marker_temp && !matches!(name, "marker.json" | "quarantine" | "staging") {
            return Err(format!(
                "GOVERNANCE_RESET_CLEANUP_FAILED:unexpected transaction entry {}",
                entry.path().display()
            ));
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
            format!(
                "GOVERNANCE_RESET_CLEANUP_FAILED:transaction entry {}:{error}",
                entry.path().display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "GOVERNANCE_RESET_CLEANUP_FAILED:symlink transaction entry {}",
                entry.path().display()
            ));
        }
        if marker_temp && !metadata.is_file() {
            return Err(format!(
                "GOVERNANCE_RESET_CLEANUP_FAILED:invalid marker temporary file {}",
                entry.path().display()
            ));
        }
    }
    Ok(())
}

fn reset_transaction_remove_marker_temps(transaction_dir: &Path) -> Result<(), String> {
    let mut entries = fs::read_dir(transaction_dir).map_err(|error| {
        format!(
            "GOVERNANCE_RESET_CLEANUP_FAILED:transaction read {}:{error}",
            transaction_dir.display()
        )
    })?;
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(|error| {
            format!(
                "GOVERNANCE_RESET_CLEANUP_FAILED:transaction read {}:{error}",
                transaction_dir.display()
            )
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !reset_transaction_is_marker_temp(name) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
            format!(
                "GOVERNANCE_RESET_CLEANUP_FAILED:marker temporary file {}:{error}",
                entry.path().display()
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "GOVERNANCE_RESET_CLEANUP_FAILED:invalid marker temporary file {}",
                entry.path().display()
            ));
        }
        fs::remove_file(entry.path()).map_err(|error| {
            format!(
                "GOVERNANCE_RESET_CLEANUP_FAILED:marker temporary file {}:{error}",
                entry.path().display()
            )
        })?;
    }
    Ok(())
}

fn reset_transaction_cleanup_committed(transaction_dir: &Path) -> Result<(), String> {
    reset_transaction_validate_cleanup_layout(transaction_dir)?;
    for relative in ["quarantine", "staging"] {
        reset_transaction_remove_path(transaction_dir, &transaction_dir.join(relative))?;
    }
    reset_transaction_remove_marker_temps(transaction_dir)?;
    reset_transaction_validate_cleanup_layout(transaction_dir)?;
    reset_transaction_remove_path(transaction_dir, &transaction_dir.join("marker.json"))?;
    match fs::remove_dir(transaction_dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "RESET_TRANSACTION_REMOVE_FAILED:{}:{error}",
            transaction_dir.display()
        )),
    }
}

fn reset_transaction_recover_unmarked(transaction_dir: &Path) -> Result<Option<bool>, String> {
    let mut entries = fs::read_dir(transaction_dir).map_err(|error| {
        format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:transaction read {}:{error}",
            transaction_dir.display()
        )
    })?;
    let mut marker_temps = Vec::new();
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(|error| {
            format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:transaction read {}:{error}",
                transaction_dir.display()
            )
        })?;
        let name = entry.file_name();
        match name.to_str() {
            Some("quarantine") | Some("staging") => {
                if !reset_transaction_directory_empty(&entry.path())? {
                    return Err(
                        "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing marker with transaction data"
                            .into(),
                    );
                }
            }
            Some(name) if reset_transaction_is_marker_temp(name) => {
                let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                    format!(
                        "GOVERNANCE_RESET_RECOVERY_REQUIRED:marker temporary file {}:{error}",
                        entry.path().display()
                    )
                })?;
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(format!(
                        "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid marker temporary file {}",
                        entry.path().display()
                    ));
                }
                marker_temps.push(entry.path());
            }
            _ => {
                return Err(format!(
                    "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing marker with unexpected transaction entry {}",
                    entry.path().display()
                ));
            }
        }
    }
    for marker_temp in marker_temps {
        fs::remove_file(&marker_temp).map_err(|error| {
            format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:marker temporary file {}:{error}",
                marker_temp.display()
            )
        })?;
    }
    reset_transaction_remove_path(
        transaction_dir.parent().unwrap_or(transaction_dir),
        transaction_dir,
    )?;
    Ok(Some(false))
}

fn reset_transaction_target_value(
    target: &ResetTransactionTarget,
    transaction_dir: &Path,
) -> Value {
    let backup = target
        .backup
        .strip_prefix(transaction_dir)
        .unwrap_or(&target.backup)
        .to_string_lossy()
        .to_string();
    let staged = target.staged.as_ref().map(|path| {
        path.strip_prefix(transaction_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string()
    });
    serde_json::json!({
        "relative": target.relative,
        "kind": target.kind,
        "original_exists": target.original_exists,
        "backup": backup,
        "staged": staged,
        "quarantined": target.quarantined,
        "published": target.published
    })
}

fn reset_transaction_marker(
    transaction_dir: &Path,
    transaction_id: &str,
    root: &Path,
    phase: &str,
    error: Option<&str>,
    targets: &[ResetTransactionTarget],
    created_dirs: &[String],
    generated_roots: &[String],
) -> Result<(), String> {
    let marker = serde_json::json!({
        "schema_version": 1,
        "transaction_id": transaction_id,
        "root": root.to_string_lossy(),
        "phase": phase,
        "error": error,
        "created_dirs": created_dirs,
        "generated_roots": generated_roots,
        "targets": targets.iter().map(|target| reset_transaction_target_value(target, transaction_dir)).collect::<Vec<_>>(),
        "updated_at": Utc::now().to_rfc3339()
    });
    reset_transaction_write_json(
        transaction_dir,
        &transaction_dir.join("marker.json"),
        &marker,
    )
}

fn reset_transaction_read_marker(transaction_dir: &Path) -> Result<Value, String> {
    let marker_path = transaction_dir.join("marker.json");
    reset_transaction_symlink_components(transaction_dir, &marker_path)?;
    if fs::symlink_metadata(&marker_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:{}",
            marker_path.display()
        ));
    }
    let text = fs::read_to_string(&marker_path).map_err(|error| {
        format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:{}:{error}",
            marker_path.display()
        )
    })?;
    serde_json::from_str(&text).map_err(|error| {
        format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:{}:{error}",
            marker_path.display()
        )
    })
}

fn reset_transaction_validate_relative(relative: &str) -> Result<(), String> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid target {relative}"
        ));
    }
    Ok(())
}

fn reset_transaction_recover(root: &Path) -> Result<Option<bool>, String> {
    let transaction_dir = reset_transaction_dir(root);
    reset_transaction_symlink_components(
        transaction_dir.parent().unwrap_or(root),
        &transaction_dir,
    )?;
    match fs::symlink_metadata(&transaction_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:{}",
                transaction_dir.display()
            ));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:transaction is not a directory {}",
                transaction_dir.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:{}:{error}",
                transaction_dir.display()
            ));
        }
    }
    let marker_path = transaction_dir.join("marker.json");
    match fs::symlink_metadata(&marker_path) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return reset_transaction_recover_unmarked(&transaction_dir)
        }
        Err(error) => {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:{}:{error}",
                marker_path.display()
            ))
        }
    }
    let marker = reset_transaction_read_marker(&transaction_dir)?;
    reset_transaction_validate_marker(root, &transaction_dir, &marker)?;
    let phase = marker
        .get("phase")
        .and_then(Value::as_str)
        .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing phase".to_string())?;
    let committed = matches!(phase, "committed" | "cleanup_failed");
    if committed {
        reset_transaction_cleanup_committed(&transaction_dir)?;
        return Ok(Some(true));
    }
    let values = marker
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing targets".to_string())?;
    for value in values.iter().rev() {
        let relative = value
            .get("relative")
            .and_then(Value::as_str)
            .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing target".to_string())?;
        let original = root.join(relative);
        let backup_rel = value
            .get("backup")
            .and_then(Value::as_str)
            .ok_or_else(|| "GOVERNANCE_RESET_RECOVERY_REQUIRED:missing backup".to_string())?;
        let backup = transaction_dir.join(backup_rel);
        if backup.exists() {
            reset_transaction_symlink_components(&transaction_dir, &backup)?;
            reset_transaction_remove_path(root, &original)?;
            fs::rename(&backup, &original).map_err(|error| {
                format!("GOVERNANCE_RESET_ROLLBACK_FAILED:{}:{error}", relative)
            })?;
        } else if value.get("original_exists").and_then(Value::as_bool) == Some(false) {
            let staged_rel = value.get("staged").and_then(Value::as_str);
            let staged_exists = staged_rel
                .map(|relative| transaction_dir.join(relative).exists())
                .unwrap_or(false);
            if value.get("published").and_then(Value::as_bool) == Some(true) || !staged_exists {
                reset_transaction_remove_path(root, &original)?;
            }
        }
    }
    let created_dirs = marker
        .get("created_dirs")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for created in created_dirs.iter().rev() {
        let target = root.join(created);
        reset_transaction_symlink_components(root, &target)?;
        match fs::remove_dir(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "GOVERNANCE_RESET_ROLLBACK_FAILED:{}:{error}",
                    created
                ))
            }
        }
    }
    reset_transaction_cleanup_committed(&transaction_dir)?;
    Ok(Some(false))
}

fn reset_transaction_fresh_project_targets() -> Vec<String> {
    let mut targets = SDK_BUNDLE_RESOURCES
        .iter()
        .filter(|(path, _, _)| {
            path.starts_with("contracts/records/") || path.starts_with("contracts/transitions/")
        })
        .map(|(path, _, _)| (*path).to_string())
        .collect::<Vec<_>>();
    let alias = "contracts/transitions/zone-transition-manifest.json".to_string();
    if !targets.iter().any(|target| target == &alias) {
        targets.push(alias);
    }
    targets
}

fn reset_transaction_expected_target_relatives(generated_roots: &[String]) -> BTreeSet<String> {
    let mut expected = BTreeSet::new();
    expected.insert(".appsdk".to_string());
    expected.insert(".appsdk-control".to_string());
    expected.insert(".gitignore".to_string());
    expected.extend(reset_transaction_quarantine_generated_roots(
        generated_roots,
    ));
    expected.extend(reset_transaction_fresh_project_targets());
    expected
}

fn reset_transaction_add_target(
    root: &Path,
    transaction_dir: &Path,
    relative: String,
    kind: &'static str,
    staged: Option<PathBuf>,
    targets: &mut Vec<ResetTransactionTarget>,
) -> Result<(), String> {
    let original = root.join(&relative);
    let metadata = match fs::symlink_metadata(&original) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(format!("GOVERNANCE_PATH_SYMLINK:{relative}"));
            }
            if (kind == "dir" && !metadata.is_dir()) || (kind == "file" && !metadata.is_file()) {
                let error = match kind {
                    "dir" => "GOVERNANCE_PATH_NOT_DIRECTORY",
                    "file" => "GOVERNANCE_PATH_NOT_FILE",
                    _ => "GOVERNANCE_PATH_NOT_TARGET",
                };
                return Err(format!("{error}:{relative}"));
            }
            Some(metadata)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "GOVERNANCE_PATH_METADATA_FAILED:{relative}:{error}"
            ))
        }
    };
    reset_transaction_symlink_components(root, &original)?;
    let index = targets.len();
    targets.push(ResetTransactionTarget {
        relative,
        original,
        backup: transaction_dir
            .join("quarantine")
            .join(format!("target-{index}")),
        staged,
        kind,
        original_exists: metadata.is_some(),
        quarantined: false,
        published: false,
    });
    Ok(())
}

fn reset_transaction_build_targets(
    root: &Path,
    transaction_dir: &Path,
    staging_root: &Path,
    generated_roots: &[String],
) -> Result<Vec<ResetTransactionTarget>, String> {
    let mut targets = Vec::new();
    let generated_roots = reset_transaction_quarantine_generated_roots(generated_roots);
    reset_transaction_add_target(
        root,
        transaction_dir,
        ".appsdk".into(),
        "dir",
        Some(staging_root.join(".appsdk")),
        &mut targets,
    )?;
    reset_transaction_add_target(
        root,
        transaction_dir,
        ".appsdk-control".into(),
        "dir",
        Some(staging_root.join(".appsdk-control")),
        &mut targets,
    )?;
    for relative in generated_roots {
        let staged = (relative == "generated").then(|| staging_root.join(&relative));
        if !targets.iter().any(|target| target.relative == relative) {
            reset_transaction_add_target(
                root,
                transaction_dir,
                relative,
                "dir",
                staged,
                &mut targets,
            )?;
        }
    }
    for relative in reset_transaction_fresh_project_targets() {
        if !targets.iter().any(|target| target.relative == relative) {
            reset_transaction_add_target(
                root,
                transaction_dir,
                relative.clone(),
                "file",
                Some(staging_root.join(&relative)),
                &mut targets,
            )?;
        }
    }
    reset_transaction_add_target(
        root,
        transaction_dir,
        ".gitignore".into(),
        "file",
        Some(staging_root.join(".gitignore")),
        &mut targets,
    )?;
    Ok(targets)
}

fn reset_transaction_quarantine_generated_roots(generated_roots: &[String]) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    for relative in generated_roots {
        let normalized = relative.trim_end_matches('/').to_string();
        if roots.iter().any(|existing| {
            normalized == *existing || normalized.starts_with(&format!("{existing}/"))
        }) {
            continue;
        }
        roots.retain(|existing| !existing.starts_with(&format!("{normalized}/")));
        roots.push(normalized);
    }
    roots.sort();
    roots
}

fn reset_transaction_build_staging(
    root: &Path,
    transaction_dir: &Path,
    generated_roots: &[String],
    transaction_id: &str,
    branch: &str,
) -> Result<(), String> {
    let staging_root = transaction_dir.join("staging");
    reset_transaction_symlink_components(transaction_dir, &staging_root)?;
    fs::create_dir_all(&staging_root)
        .map_err(|error| format!("GOVERNANCE_RESET_STAGING_CREATE_FAILED:{error}"))?;
    let binary = env::current_exe()
        .map_err(|error| format!("GOVERNANCE_RESET_STAGING_BINARY_FAILED:{error}"))?;
    let output = Command::new(binary)
        .args(["new", staging_root.to_str().unwrap_or("")])
        .env_remove("TMUX_PANE")
        .output()
        .map_err(|error| format!("GOVERNANCE_RESET_STAGING_BUILD_FAILED:{error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if detail.len() > 512 {
            &detail[..512]
        } else {
            detail.as_str()
        };
        return Err(format!(
            "GOVERNANCE_RESET_STAGING_BUILD_FAILED:exit={}:{}",
            output.status.code().unwrap_or(-1),
            detail
        ));
    }
    // `appsdk new` bootstraps the stable project scaffold.  Fresh reset also
    // replaces every root contract declared by the current bundle, including
    // record contracts added after the scaffold template was published.  Write
    // those canonical bytes into staging before any project path is moved.
    for &(relative, _, content) in SDK_BUNDLE_RESOURCES.iter().filter(|(path, _, _)| {
        path.starts_with("contracts/records/") || path.starts_with("contracts/transitions/")
    }) {
        reset_transaction_write_bytes(
            transaction_dir,
            &staging_root.join(relative),
            content.as_bytes(),
        )?;
    }
    reset_transaction_write_bytes(
        transaction_dir,
        &staging_root.join("contracts/transitions/zone-transition-manifest.json"),
        CANONICAL_ZONE_TRANSITION_CONTRACT.as_bytes(),
    )?;
    let gitignore = root.join(".gitignore");
    if fs::symlink_metadata(&gitignore)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("GOVERNANCE_PATH_SYMLINK:gitignore".into());
    }
    let content = if gitignore.exists() {
        fs::read_to_string(&gitignore)
            .map_err(|error| format!("GOVERNANCE_RESET_GITIGNORE_READ_FAILED:{error}"))?
    } else {
        String::new()
    };
    let updated = render_appsdk_gitignore(content)?;
    reset_transaction_write_bytes(
        transaction_dir,
        &staging_root.join(".gitignore"),
        updated.as_bytes(),
    )?;
    let mut removed = vec![".appsdk".to_string(), ".appsdk-control".to_string()];
    removed.extend(generated_roots.iter().cloned());
    let reset_record = serde_json::json!({
        "schema_version": 1,
        "reset_id": transaction_id,
        "transaction_id": transaction_id,
        "mode": "fresh_init",
        "preserved": ["business_source", "runtime_data", "active", "protected"],
        "removed": removed,
        "branch": branch,
        "created_at": Utc::now().to_rfc3339()
    });
    reset_transaction_write_json(
        transaction_dir,
        &staging_root.join(".appsdk/records/reset-governance-record.json"),
        &reset_record,
    )?;
    Ok(())
}

fn reset_transaction_rollback(
    root: &Path,
    transaction_dir: &Path,
    transaction_id: &str,
    targets: &[ResetTransactionTarget],
    created_dirs: &[String],
    generated_roots: &[String],
    cause: &str,
) -> Result<(), String> {
    let mut first_error = None;
    for target in targets.iter().rev() {
        let result = if target.backup.exists() {
            reset_transaction_symlink_components(&transaction_dir, &target.backup).and_then(|_| {
                reset_transaction_remove_path(root, &target.original).and_then(|_| {
                    fs::rename(&target.backup, &target.original).map_err(|error| {
                        format!(
                            "GOVERNANCE_RESET_ROLLBACK_FAILED:{}:{error}",
                            target.relative
                        )
                    })
                })
            })
        } else if !target.original_exists && target.published {
            reset_transaction_remove_path(root, &target.original)
        } else {
            Ok(())
        };
        if let Err(error) = result {
            first_error.get_or_insert(error);
        }
    }
    for created in created_dirs.iter().rev() {
        let target = root.join(created);
        if let Err(error) = reset_transaction_symlink_components(root, &target).and_then(|_| {
            match fs::remove_dir(&target) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!(
                    "GOVERNANCE_RESET_ROLLBACK_FAILED:{}:{error}",
                    created
                )),
            }
        }) {
            first_error.get_or_insert(error);
        }
    }
    if let Err(error) =
        reset_transaction_remove_path(transaction_dir, &transaction_dir.join("staging"))
    {
        first_error.get_or_insert(error);
    }
    if let Some(error) = first_error {
        let combined = format!("{cause};{error}");
        let _ = reset_transaction_marker(
            transaction_dir,
            transaction_id,
            root,
            "rollback_failed",
            Some(&combined),
            targets,
            created_dirs,
            generated_roots,
        );
        return Err(format!("GOVERNANCE_RESET_ROLLBACK_FAILED:{combined}"));
    }
    if let Err(error) =
        reset_transaction_remove_path(transaction_dir.parent().unwrap_or(root), transaction_dir)
    {
        let combined = format!("{cause};{error}");
        let _ = reset_transaction_marker(
            transaction_dir,
            transaction_id,
            root,
            "rollback_failed",
            Some(&combined),
            targets,
            created_dirs,
            generated_roots,
        );
        return Err(format!("GOVERNANCE_RESET_ROLLBACK_FAILED:{combined}"));
    }
    Ok(())
}

fn reset_transaction_rollback_or_combine(
    root: &Path,
    transaction_dir: &Path,
    transaction_id: &str,
    targets: &[ResetTransactionTarget],
    created_dirs: &[String],
    generated_roots: &[String],
    cause: &str,
) -> String {
    match reset_transaction_rollback(
        root,
        transaction_dir,
        transaction_id,
        targets,
        created_dirs,
        generated_roots,
        cause,
    ) {
        Ok(()) => cause.to_string(),
        Err(error) => format!("{cause};{error}"),
    }
}

fn reset_transaction_fresh(
    root: &Path,
    branch: &str,
    generated_roots: &[String],
) -> Result<(), String> {
    let transaction_dir = reset_transaction_dir(root);
    reset_transaction_symlink_components(
        transaction_dir.parent().unwrap_or(root),
        &transaction_dir,
    )?;
    match fs::symlink_metadata(&transaction_dir) {
        Ok(_) => {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:{}",
                transaction_dir.display()
            ));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "GOVERNANCE_RESET_RECOVERY_REQUIRED:{}:{error}",
                transaction_dir.display()
            ));
        }
    }
    fs::create_dir_all(&transaction_dir)
        .map_err(|error| format!("GOVERNANCE_RESET_TRANSACTION_CREATE_FAILED:{error}"))?;
    let transaction_id = reset_transaction_id()?;
    let empty_targets = Vec::new();
    let empty_created = Vec::new();
    reset_transaction_marker(
        &transaction_dir,
        &transaction_id,
        root,
        "building",
        None,
        &empty_targets,
        &empty_created,
        generated_roots,
    )?;
    fs::create_dir_all(transaction_dir.join("quarantine"))
        .map_err(|error| format!("GOVERNANCE_RESET_TRANSACTION_CREATE_FAILED:{error}"))?;
    if let Err(error) = reset_transaction_build_staging(
        root,
        &transaction_dir,
        generated_roots,
        &transaction_id,
        branch,
    ) {
        let _ = reset_transaction_marker(
            &transaction_dir,
            &transaction_id,
            root,
            "build_failed",
            Some(&error),
            &empty_targets,
            &empty_created,
            generated_roots,
        );
        return Err(error);
    }
    let staging_root = transaction_dir.join("staging");
    let mut targets = match reset_transaction_build_targets(
        root,
        &transaction_dir,
        &staging_root,
        generated_roots,
    ) {
        Ok(targets) => targets,
        Err(error) => {
            let _ = reset_transaction_marker(
                &transaction_dir,
                &transaction_id,
                root,
                "preflight_failed",
                Some(&error),
                &empty_targets,
                &empty_created,
                generated_roots,
            );
            return Err(error);
        }
    };
    let created_dirs = reset_transaction_created_dirs(root, &targets)?;
    reset_transaction_marker(
        &transaction_dir,
        &transaction_id,
        root,
        "prepared",
        None,
        &targets,
        &created_dirs,
        generated_roots,
    )?;
    for index in 0..targets.len() {
        if !targets[index].original_exists {
            continue;
        }
        reset_transaction_symlink_components(root, &targets[index].original)?;
        reset_transaction_symlink_components(&transaction_dir, &targets[index].backup)?;
        if let Err(error) = fs::rename(&targets[index].original, &targets[index].backup) {
            let cause = format!(
                "GOVERNANCE_RESET_QUARANTINE_FAILED:{}:{error}",
                targets[index].relative
            );
            let failure = reset_transaction_rollback_or_combine(
                root,
                &transaction_dir,
                &transaction_id,
                &targets,
                &created_dirs,
                generated_roots,
                &cause,
            );
            return Err(failure);
        }
        targets[index].quarantined = true;
        if let Err(error) = reset_transaction_marker(
            &transaction_dir,
            &transaction_id,
            root,
            "quarantining",
            None,
            &targets,
            &created_dirs,
            generated_roots,
        ) {
            let failure = reset_transaction_rollback_or_combine(
                root,
                &transaction_dir,
                &transaction_id,
                &targets,
                &created_dirs,
                generated_roots,
                &error,
            );
            return Err(failure);
        }
    }
    for created in &created_dirs {
        let target = root.join(created);
        if let Err(error) = reset_transaction_symlink_components(root, &target) {
            let cause = format!("GOVERNANCE_RESET_CREATED_DIR_FAILED:{created}:{error}");
            let failure = reset_transaction_rollback_or_combine(
                root,
                &transaction_dir,
                &transaction_id,
                &targets,
                &created_dirs,
                generated_roots,
                &cause,
            );
            return Err(failure);
        }
        if !target.exists() {
            if let Err(error) = fs::create_dir_all(&target) {
                let cause = format!("GOVERNANCE_RESET_CREATED_DIR_FAILED:{created}:{error}");
                let failure = reset_transaction_rollback_or_combine(
                    root,
                    &transaction_dir,
                    &transaction_id,
                    &targets,
                    &created_dirs,
                    generated_roots,
                    &cause,
                );
                return Err(failure);
            }
        }
    }
    for index in 0..targets.len() {
        let Some(staged) = targets[index].staged.clone() else {
            continue;
        };
        reset_transaction_symlink_components(&transaction_dir, &staged)?;
        reset_transaction_symlink_components(root, &targets[index].original)?;
        if !staged.exists() {
            let cause = format!(
                "GOVERNANCE_RESET_STAGED_TARGET_MISSING:{}",
                targets[index].relative
            );
            let failure = reset_transaction_rollback_or_combine(
                root,
                &transaction_dir,
                &transaction_id,
                &targets,
                &created_dirs,
                generated_roots,
                &cause,
            );
            return Err(failure);
        }
        if let Err(error) = fs::rename(&staged, &targets[index].original) {
            let cause = format!(
                "GOVERNANCE_RESET_PUBLISH_FAILED:{}:{error}",
                targets[index].relative
            );
            let failure = reset_transaction_rollback_or_combine(
                root,
                &transaction_dir,
                &transaction_id,
                &targets,
                &created_dirs,
                generated_roots,
                &cause,
            );
            return Err(failure);
        }
        targets[index].published = true;
        if let Err(error) = reset_transaction_marker(
            &transaction_dir,
            &transaction_id,
            root,
            "publishing",
            None,
            &targets,
            &created_dirs,
            generated_roots,
        ) {
            let failure = reset_transaction_rollback_or_combine(
                root,
                &transaction_dir,
                &transaction_id,
                &targets,
                &created_dirs,
                generated_roots,
                &error,
            );
            return Err(failure);
        }
    }
    reset_transaction_marker(
        &transaction_dir,
        &transaction_id,
        root,
        "committed",
        None,
        &targets,
        &created_dirs,
        generated_roots,
    )?;
    for target in &targets {
        if let Err(error) = reset_transaction_remove_path(&transaction_dir, &target.backup) {
            let cleanup = format!("GOVERNANCE_RESET_CLEANUP_FAILED:{}", error);
            let _ = reset_transaction_marker(
                &transaction_dir,
                &transaction_id,
                root,
                "cleanup_failed",
                Some(&cleanup),
                &targets,
                &created_dirs,
                generated_roots,
            );
            return Err(cleanup);
        }
    }
    if let Err(error) = reset_transaction_cleanup_committed(&transaction_dir) {
        let cleanup = format!("GOVERNANCE_RESET_CLEANUP_FAILED:{error}");
        let _ = reset_transaction_marker(
            &transaction_dir,
            &transaction_id,
            root,
            "cleanup_failed",
            Some(&cleanup),
            &targets,
            &created_dirs,
            generated_roots,
        );
        return Err(cleanup);
    }
    Ok(())
}

fn pin_lock(root: &Path, binary: &Path) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_no_symlink_components(root, &root.join(".appsdk"), "appsdk_control");
    let mut project = read_project(root);
    let project_version = required_str(&project, "/sdk/version", "INVALID_SDK_CONTRACT");
    if !matches!(project_version, "0.1.3" | "0.1.4" | "0.1.5" | "0.1.6") {
        fail(format!(
            "UNSUPPORTED_SDK_MIGRATION:{}:0.1.6",
            project_version
        ));
    }
    let previous_bundle_digest = {
        let record_path = sdk_map_migration_root(root).join("record.json");
        if !record_path.is_file() {
            None
        } else {
            let record: Value = serde_json::from_str(
                &fs::read_to_string(&record_path)
                    .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD")),
            )
            .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD"));
            migration_bundle_transition_digest(root, &record)
        }
    };
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
    reconcile_authoring_bundle_manifest(root);
    if matches!(project_version, "0.1.3" | "0.1.4") {
        write_legacy_migration_step(root, project_version);
        project["sdk"]["version"] = Value::String("0.1.5".into());
        write_project(root, &project);
    }
    let migrated_project = read_project(root);
    migrate_governance_maps(root, &migrated_project, "0.1.5");
    install_current_record_contracts(root);
    project = migrated_project;
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
    if let Some(previous_bundle_digest) = previous_bundle_digest {
        lock.insert(
            "previous_bundle_digest".into(),
            Value::String(previous_bundle_digest),
        );
    }
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

fn reset_root_first_segment(relative: &str) -> &str {
    relative.split('/').next().unwrap_or(relative)
}

fn reset_root_conflicts_with_reserved(relative: &str, case_insensitive: bool) -> bool {
    let protected = [
        ".appsdk",
        ".appsdk-control",
        ".git",
        ".agent-collab",
        "active",
        "protected",
        "business",
    ];
    let first = reset_root_first_segment(relative);
    protected.iter().any(|reserved| {
        if case_insensitive {
            first.eq_ignore_ascii_case(reserved)
        } else {
            first == *reserved
        }
    })
}

fn reset_root_filesystem_is_case_insensitive(root: &Path) -> bool {
    // A governance reset always has `.appsdk` present when it reaches this
    // check. Comparing its canonical path with a case variant gives us the
    // root filesystem's actual behavior without creating probe files.
    match (
        fs::canonicalize(root.join(".appsdk")),
        fs::canonicalize(root.join(".APPSDK")),
    ) {
        (Ok(actual), Ok(variant)) => actual == variant,
        _ => false,
    }
}

fn reset_generated_roots(root: &Path) -> Result<Vec<String>, String> {
    let project = project_file(root);
    if fs::symlink_metadata(&project)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("GOVERNANCE_PATH_SYMLINK:project".into());
    }
    let text = fs::read_to_string(&project)
        .map_err(|_| format!("PROJECT_CONTRACT_MISSING:{}", project.display()))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|_| "INVALID_PROJECT_CONTRACT".to_string())?;
    let roots = reset_transaction_parse_generated_roots(
        &value,
        reset_root_filesystem_is_case_insensitive(root),
    )?;
    for relative in roots.iter().skip(1) {
        let path = root.join(relative);
        if fs::symlink_metadata(root)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err("GOVERNANCE_PATH_SYMLINK:generated_root".into());
        }
        let relative_path = path
            .strip_prefix(root)
            .map_err(|_| "GOVERNANCE_PATH_ESCAPE:generated_root".to_string())?;
        let mut current = root.to_path_buf();
        for component in relative_path.components() {
            current.push(component.as_os_str());
            if fs::symlink_metadata(&current)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                return Err("GOVERNANCE_PATH_SYMLINK:generated_root".into());
            }
        }
    }
    Ok(roots)
}

fn reset_governance(root: &Path, discard_legacy: bool) {
    reset_governance_internal(root, discard_legacy, false).unwrap_or_else(|error| fail(error));
}

fn reset_governance_internal(
    root: &Path,
    discard_legacy: bool,
    fresh_init: bool,
) -> Result<(), String> {
    if !discard_legacy {
        return Err("RESET_REQUIRES_DISCARD_LEGACY_CONFIRMATION".into());
    }
    assert_project_root_safe(root);
    let reset_record = root
        .join(".appsdk")
        .join("records")
        .join("reset-governance-record.json");

    let branch = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or(""),
            "branch",
            "--show-current",
        ])
        .output()
        .map_err(|_| "RESET_GIT_WORKTREE_REQUIRED".to_string())?;
    if !branch.status.success() {
        return Err("RESET_GIT_WORKTREE_REQUIRED".into());
    }
    let branch = String::from_utf8_lossy(&branch.stdout).trim().to_string();
    if branch.is_empty() || branch == "main" || branch == "master" {
        return Err("RESET_REQUIRES_NON_MAIN_WORKTREE".into());
    }
    if reset_record.exists() && !fresh_init {
        println!("governance reset already applied");
        return Ok(());
    }

    let _fresh_lock = if fresh_init {
        Some(reset_transaction_acquire_lock(root)?)
    } else {
        None
    };
    if fresh_init {
        match reset_transaction_recover(root) {
            Ok(Some(true)) => {
                println!("governance fresh init already applied");
                return Ok(());
            }
            Ok(Some(false)) => return Err("GOVERNANCE_RESET_RECOVERED_RETRY".into()),
            Ok(None) => {}
            Err(error) => return Err(error),
        }
    }
    let status = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or(""),
            "status",
            "--porcelain",
            "--",
            ".",
        ])
        .output()
        .map_err(|_| "RESET_GIT_WORKTREE_REQUIRED".to_string())?;
    if !status.status.success() {
        return Err("RESET_GIT_WORKTREE_REQUIRED".into());
    }
    if !status.stdout.is_empty() {
        return Err("RESET_REQUIRES_CLEAN_WORKTREE".into());
    }
    let generated_roots = reset_generated_roots(root)?;
    if fresh_init {
        reset_transaction_fresh(root, &branch, &generated_roots)?;
        println!("governance fresh init applied");
        return Ok(());
    }
    let mut removed = vec![".appsdk".to_string(), ".appsdk-control".to_string()];
    removed.extend(generated_roots.iter().cloned());
    let reset_targets = [".appsdk", ".appsdk-control"]
        .into_iter()
        .chain(generated_roots.iter().map(String::as_str))
        .map(|relative| (relative.to_string(), root.join(relative)))
        .collect::<Vec<_>>();
    // Validate every deletion target before removing the first one. A later
    // symlink or reserved-root failure must leave the legacy control plane
    // untouched.
    for (relative, target) in &reset_targets {
        match fs::symlink_metadata(target) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                fail(format!("GOVERNANCE_PATH_SYMLINK:{}", relative));
            }
            Ok(metadata) if !metadata.is_dir() => {
                fail(format!("GOVERNANCE_PATH_NOT_DIRECTORY:{}", relative));
            }
            Ok(_) => {}
            Err(error) if error.kind() != ErrorKind::NotFound => {
                fail(format!("GOVERNANCE_PATH_METADATA_FAILED:{}", relative));
            }
            Err(_) => {}
        }
    }
    for (_, target) in reset_targets {
        if target.exists() {
            fs::remove_dir_all(&target).unwrap_or_else(|_| fail("GOVERNANCE_RESET_FAILED"));
        }
    }

    ensure_governance_layout(root);
    write_project_scaffold(root);
    install_bundle_resources(root);
    write_current_sdk_lock(root);
    atomic_write_json(
        &reset_record,
        &serde_json::json!({
            "schema_version": 1,
            "reset_id": format!("reset-{}", std::process::id()),
            "mode": if fresh_init {
                "fresh_init"
            } else {
                "discard_legacy_control_plane"
            },
            "preserved": ["business_source", "runtime_data", "active", "protected"],
            "removed": removed,
            "branch": branch,
            "created_at": Utc::now().to_rfc3339()
        }),
        "GOVERNANCE_RESET_RECORD_FAILED",
    );
    if fresh_init {
        println!("governance fresh init applied");
    } else {
        println!("governance reset applied");
    }
    Ok(())
}

fn locate_git_bug_binary() -> Result<PathBuf, String> {
    if let Ok(path) = env::var("GIT_BUG_BIN") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
    }
    if let Ok(output) = Command::new("which").arg("git-bug").output() {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !s.is_empty() {
                return Ok(PathBuf::from(s));
            }
        }
    }
    if let Ok(home) = env::var("HOME") {
        let p = PathBuf::from(home).join(".local/bin/git-bug");
        if p.exists() {
            return Ok(p);
        }
    }
    Err("GIT_BUG_NOT_FOUND: please run `appsdk setup-deps` to install git-bug".into())
}

fn bug_record_matches_identity(record: &Value, issue_id: &str) -> bool {
    record.get("human_id").and_then(Value::as_str) == Some(issue_id)
        || record.get("id").and_then(Value::as_str) == Some(issue_id)
}

fn query_bug_record(root: &Path, issue_id: &str) -> Result<Value, String> {
    let git_bug = locate_git_bug_binary()?;
    let run = |dir: &Path| {
        Command::new(&git_bug)
            .args(["bug", "show", issue_id, "-f", "json"])
            .current_dir(dir)
            .output()
    };
    let output = run_git_bug_read(|| run(root), true)
        .map_err(|error| format!("BUG_TRIAGE_QUERY_EXECUTION_FAILED:{error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if detail.is_empty() {
            format!(
                "BUG_TRIAGE_QUERY_FAILED:{}:exit={}",
                issue_id,
                output.status.code().unwrap_or(-1)
            )
        } else {
            format!("BUG_TRIAGE_QUERY_FAILED:{}:{}", issue_id, detail)
        });
    }
    let record: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("BUG_TRIAGE_QUERY_INVALID_JSON:{}:{error}", issue_id))?;
    if !bug_record_matches_identity(&record, issue_id) {
        return Err(format!("BUG_TRIAGE_QUERY_IDENTITY_MISMATCH:{issue_id}"));
    }
    Ok(record)
}

fn git_bug_close_event(root: &Path, record: &Value) -> Option<Value> {
    if record.get("status").and_then(Value::as_str) != Some("closed") {
        return None;
    }
    let bug_id = record.get("id").and_then(Value::as_str)?;
    let ref_name = format!("refs/bugs/{bug_id}^{{commit}}");
    let tip = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "--verify", &ref_name])
            .current_dir(root)
            .output()
            .ok()?
            .stdout,
    )
    .ok()?
    .trim()
    .to_string();
    if tip.is_empty() {
        return None;
    }
    let commit = String::from_utf8(
        Command::new("git")
            .args(["cat-file", "-p", &tip])
            .current_dir(root)
            .output()
            .ok()?
            .stdout,
    )
    .ok()?;
    let tree = commit.lines().find_map(|line| line.strip_prefix("tree "))?;
    let ops_blob = String::from_utf8(
        Command::new("git")
            .args(["ls-tree", "-r", tree])
            .current_dir(root)
            .output()
            .ok()?
            .stdout,
    )
    .ok()?
    .lines()
    .find_map(|line| {
        let mut parts = line.split_whitespace();
        let blob = parts.nth(2)?;
        (parts.next() == Some("ops")).then(|| blob.to_string())
    })?;
    let ops: Value = serde_json::from_slice(
        &Command::new("git")
            .args(["cat-file", "-p", &ops_blob])
            .current_dir(root)
            .output()
            .ok()?
            .stdout,
    )
    .ok()?;
    let close_op = ops.get("ops").and_then(Value::as_array).and_then(|items| {
        items.iter().find(|item| {
            item.get("type").and_then(Value::as_u64) == Some(4)
                && item.get("status").and_then(Value::as_u64) == Some(2)
        })
    })?;
    let comment_id = record
        .get("comments")
        .and_then(Value::as_array)
        .and_then(|comments| comments.last())
        .and_then(|comment| comment.get("id"))
        .and_then(Value::as_str)?;
    Some(serde_json::json!({
        "event_id": tip,
        "action": "close",
        "comment_id": comment_id,
        "producer": {
            "adapter": "appsdk",
            "identity": "appsdk::bug-close"
        },
        "metadata": {
            "bug_id": bug_id,
            "status": "closed",
            "timestamp": close_op.get("timestamp")
        }
    }))
}

fn assert_bug_tracker_triage_evidence(
    worktree: &Value,
    issue_id: &str,
    real_query_root: Option<&Path>,
    require_binding: bool,
) {
    let triage = worktree.get("bug_triage");
    let legacy_issue = issue_id.starts_with("legacy-");
    let exempt_issue = issue_id.is_empty() || issue_id == "none" || legacy_issue;
    if triage.is_none() {
        if !exempt_issue {
            fail("BUG_TRIAGE_MISSING");
        }
        return;
    }
    if issue_id.is_empty() || issue_id == "none" {
        fail("BUG_TRIAGE_UNEXPECTED_FOR_EXEMPT_ISSUE");
    }

    let triage = triage.unwrap();
    if !triage.is_object() {
        fail("INVALID_BUG_TRIAGE");
    }
    let mode = triage.get("mode").and_then(Value::as_str).unwrap_or("");
    if !matches!(mode, "new_confirmed" | "reopened" | "historical_legacy") {
        fail("BUG_TRIAGE_MODE_INVALID");
    }
    if triage.get("query_executed") != Some(&Value::Bool(true)) {
        fail("BUG_TRIAGE_QUERY_MISSING");
    }
    let query = triage.get("query").and_then(Value::as_str).unwrap_or("");
    if query.is_empty() || !query.split_whitespace().any(|token| token == issue_id) {
        fail("BUG_TRIAGE_QUERY_UNBOUND");
    }
    let reopened_from = triage
        .get("reopened_from_issue_id")
        .unwrap_or_else(|| fail("BUG_TRIAGE_REOPENED_SOURCE_MISSING"));
    let reopened_from_id = match reopened_from {
        Value::Null => None,
        Value::String(value) => Some(value.as_str()),
        _ => fail("BUG_TRIAGE_REOPENED_SOURCE_INVALID"),
    };
    if mode == "historical_legacy" {
        if !legacy_issue {
            fail("BUG_TRIAGE_LEGACY_ID_MISMATCH");
        }
        if reopened_from_id.is_some() {
            fail("BUG_TRIAGE_REOPENED_SOURCE_UNEXPECTED");
        }
        return;
    }
    if legacy_issue {
        fail("BUG_TRIAGE_LEGACY_MODE_INVALID");
    }
    if mode == "reopened" {
        let Some(reopened_from_id) = reopened_from_id.filter(|value| !value.is_empty()) else {
            fail("BUG_TRIAGE_REOPENED_SOURCE_MISSING");
        };
        if reopened_from_id == issue_id
            || !query
                .split_whitespace()
                .any(|token| token == reopened_from_id)
        {
            fail("BUG_TRIAGE_REOPENED_QUERY_UNBOUND");
        }
    } else if reopened_from_id.is_some() {
        fail("BUG_TRIAGE_REOPENED_SOURCE_UNEXPECTED");
    }

    let expected_binding = sha256(&canonical(&serde_json::json!({
        "issue_id": issue_id,
        "query": query,
        "mode": mode,
        "reopened_from_issue_id": reopened_from_id
    })));
    if require_binding
        && worktree
            .get("bug_triage_query_binding")
            .and_then(Value::as_str)
            != Some(expected_binding.as_str())
    {
        fail("BUG_TRIAGE_QUERY_BINDING_MISMATCH");
    }

    if let Some(root) = real_query_root {
        query_bug_record(root, issue_id).unwrap_or_else(|error| fail(error));
        if let Some(reopened_from_id) = reopened_from_id {
            query_bug_record(root, reopened_from_id).unwrap_or_else(|error| fail(error));
        }
    }
}

#[cfg(test)]
mod bug_triage_tests {
    use super::*;

    #[test]
    fn historical_legacy_triage_binds_to_legacy_issue_without_store_query() {
        let worktree = serde_json::json!({
            "bug_triage": {
                "query_executed": true,
                "query": "appsdk bug list -q legacy-issue-1",
                "mode": "historical_legacy",
                "reopened_from_issue_id": null
            }
        });

        assert_bug_tracker_triage_evidence(&worktree, "legacy-issue-1", None, false);
    }
}

fn assert_bug_tracker_solution_evidence(root: &Path, issue_id: &str, promotion: &Value) {
    if issue_id.is_empty() || issue_id == "none" || issue_id.starts_with("legacy-") {
        return;
    }

    if promotion.get("bug_closure_verified") != Some(&Value::Bool(true)) {
        fail("BUG_TRACKER_CLOSURE_NOT_VERIFIED");
    }
    let record = query_bug_record(root, issue_id)
        .unwrap_or_else(|error| fail(format!("BUG_TRACKER_CLOSURE_QUERY_FAILED:{}", error)));
    if record.get("status").and_then(Value::as_str) != Some("closed") {
        fail(format!("BUG_TRACKER_ISSUE_NOT_CLOSED:{}", issue_id));
    }
    if git_bug_close_event(root, &record).is_none() {
        fail(format!("BUG_TRACKER_CLOSE_EVENT_MISSING:{}", issue_id));
    }
    let has_solution = record
        .get("comments")
        .and_then(Value::as_array)
        .map(|comments| {
            comments.iter().any(|comment| {
                comment
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| {
                        message.contains("### Solution / Resolution")
                            || message.contains("Solution:")
                    })
            })
        })
        .unwrap_or(false);
    if !has_solution {
        fail(format!(
            "BUG_TRACKER_SOLUTION_EVIDENCE_MISSING:{}",
            issue_id
        ));
    }
}

fn setup_deps(check_only: bool) {
    if check_only {
        match locate_git_bug_binary() {
            Ok(p) => {
                let out = Command::new(&p).arg("version").output();
                let ver = out
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_else(|_| "installed".into());
                println!("{{\"git_bug\":{{\"status\":\"installed\",\"path\":\"{}\",\"version\":\"{}\"}}}}", p.display(), ver);
            }
            Err(e) => {
                fail(format!("DEPENDENCY_CHECK_FAILED:{}", e));
            }
        }
        return;
    }

    let home = env::var("HOME").unwrap_or_else(|_| fail("HOME_NOT_SET"));
    let install_dir = PathBuf::from(&home).join(".local/bin");
    fs::create_dir_all(&install_dir).unwrap_or_else(|_| fail("INSTALL_DIR_CREATE_FAILED"));
    let git_bug_target = install_dir.join("git-bug");

    let os = match env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        other => fail(format!("UNSUPPORTED_OS:{}", other)),
    };
    let arch = match env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => fail(format!("UNSUPPORTED_ARCH:{}", other)),
    };

    let version = "0.10.1";
    let binary_name = format!("git-bug_{}_{}", os, arch);
    let download_url = format!(
        "https://github.com/git-bug/git-bug/releases/download/v{}/{}",
        version, binary_name
    );

    println!("Downloading git-bug from {} ...", download_url);
    let curl_status = Command::new("curl")
        .args([
            "-fsSL",
            &download_url,
            "-o",
            git_bug_target.to_str().unwrap(),
        ])
        .status();

    let download_success = match curl_status {
        Ok(s) if s.success() => true,
        _ => {
            let wget_status = Command::new("wget")
                .args(["-qO", git_bug_target.to_str().unwrap(), &download_url])
                .status();
            wget_status.map(|s| s.success()).unwrap_or(false)
        }
    };

    if !download_success {
        fail(format!("DOWNLOAD_FAILED:{}", download_url));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(&git_bug_target) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&git_bug_target, perms);
        }
    }

    println!(
        "{{\"ok\":true,\"installed_to\":\"{}\",\"version\":\"{}\"}}",
        git_bug_target.display(),
        version
    );
}

fn resolve_upstream_repo() -> Result<PathBuf, String> {
    let candidate = if let Some(value) = env::var_os("APPSDK_ROOT") {
        let value = value.to_string_lossy().trim().to_string();
        if value.is_empty() {
            return Err("APPSDK_UPSTREAM_REPO_NOT_FOUND: APPSDK_ROOT is empty".into());
        }
        PathBuf::from(value)
    } else {
        let home = env::var_os("HOME")
            .ok_or_else(|| "APPSDK_UPSTREAM_REPO_NOT_FOUND: set APPSDK_ROOT or HOME".to_string())?;
        PathBuf::from(home).join("Documents/github/appsdk")
    };

    if !candidate.exists() {
        return Err(format!(
            "APPSDK_UPSTREAM_REPO_NOT_FOUND: expected AppSDK repository at {} (set APPSDK_ROOT)",
            candidate.display()
        ));
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(&candidate)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| {
            format!(
                "APPSDK_UPSTREAM_REPO_INVALID:{}:{}",
                candidate.display(),
                error
            )
        })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "APPSDK_UPSTREAM_REPO_INVALID:{}{}",
            candidate.display(),
            if detail.is_empty() {
                String::new()
            } else {
                format!(":{}", detail)
            }
        ));
    }

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        return Err(format!(
            "APPSDK_UPSTREAM_REPO_INVALID:{}:git returned no repository root",
            candidate.display()
        ));
    }
    fs::canonicalize(Path::new(&root)).map_err(|error| {
        format!(
            "APPSDK_UPSTREAM_REPO_INVALID:{}:{}",
            candidate.display(),
            error
        )
    })
}

fn select_bug_store(root: &Path, explicit_upstream: bool) -> Result<PathBuf, String> {
    if explicit_upstream {
        resolve_upstream_repo()
    } else {
        Ok(root.to_path_buf())
    }
}

fn run_git_bug_read<F>(mut run: F, expect_output: bool) -> std::io::Result<Output>
where
    F: FnMut() -> std::io::Result<Output>,
{
    const MAX_ATTEMPTS: usize = 5;
    for attempt in 0..MAX_ATTEMPTS {
        let output = run()?;
        if (output.status.success() && (!expect_output || !output.stdout.is_empty()))
            || attempt + 1 == MAX_ATTEMPTS
        {
            return Ok(output);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        // git-bug 0.10.1 can expose an empty lock owner as this parse error
        // while another read is releasing the repository lock.
        if !stderr.contains("already locked")
            && !stderr.contains("git-bug/lock")
            && !stderr.contains("strconv.Atoi: parsing \"\": invalid syntax")
            && !(expect_output && output.status.success())
        {
            return Ok(output);
        }
        thread::sleep(Duration::from_millis(25 * (attempt as u64 + 1)));
    }
    unreachable!("read retry loop always returns an output")
}

fn handle_bug_command<I>(root: &Path, mut args: I)
where
    I: Iterator<Item = String>,
{
    let sub = args
        .next()
        .unwrap_or_else(|| fail("USAGE: appsdk bug <new|list|show|comment|close|webui> [options]"));

    let git_bug = locate_git_bug_binary().unwrap_or_else(|e| fail(e));

    let ensure_identity = |target_dir: &Path| {
        let user_list = Command::new(&git_bug)
            .args(["user", "-f", "json"])
            .current_dir(target_dir)
            .output();
        if let Ok(out) = user_list {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if stdout.is_empty() || stdout == "[]" || stdout == "null" {
                let name_out = Command::new("git")
                    .args(["-C", target_dir.to_str().unwrap(), "config", "user.name"])
                    .output();
                let email_out = Command::new("git")
                    .args(["-C", target_dir.to_str().unwrap(), "config", "user.email"])
                    .output();
                let name = name_out
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default();
                let email = email_out
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default();
                let user_name = if name.is_empty() {
                    "AppSDK User".to_string()
                } else {
                    name
                };
                let user_email = if email.is_empty() {
                    "user@appsdk.local".to_string()
                } else {
                    email
                };

                let _ = Command::new(&git_bug)
                    .args([
                        "user",
                        "new",
                        "-n",
                        &user_name,
                        "-e",
                        &user_email,
                        "--non-interactive",
                    ])
                    .current_dir(target_dir)
                    .output();
            }
        }
    };

    match sub.as_str() {
        "new" => {
            let mut title: Option<String> = None;
            let mut message: Option<String> = None;
            let mut labels: Vec<String> = Vec::new();
            let mut upstream = false;

            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-t" | "--title" => {
                        title = Some(args.next().unwrap_or_else(|| fail("MISSING_TITLE_ARG")));
                    }
                    "-m" | "--message" => {
                        message = Some(args.next().unwrap_or_else(|| fail("MISSING_MESSAGE_ARG")));
                    }
                    "-l" | "--label" => {
                        let l = args.next().unwrap_or_else(|| fail("MISSING_LABEL_ARG"));
                        for item in l.split(',') {
                            let trimmed = item.trim();
                            if !trimmed.is_empty() {
                                labels.push(trimmed.to_string());
                            }
                        }
                    }
                    "--upstream" => {
                        upstream = true;
                    }
                    _ => fail(format!("UNKNOWN_BUG_NEW_OPTION:{}", arg)),
                }
            }

            let title_str = title.unwrap_or_else(|| {
                fail(
                    "USAGE: appsdk bug new -t <title> -m <message> [--label <labels>] [--upstream]",
                )
            });
            let message_str = message.unwrap_or_else(|| "".to_string());

            let work_dir = if upstream {
                resolve_upstream_repo().unwrap_or_else(|error| fail(error))
            } else {
                root.to_path_buf()
            };

            ensure_identity(&work_dir);

            let mut cmd = Command::new(&git_bug);
            cmd.args([
                "bug",
                "new",
                "-t",
                &title_str,
                "-m",
                &message_str,
                "--non-interactive",
            ]);
            cmd.current_dir(&work_dir);

            let output = cmd
                .output()
                .unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));
            if !output.status.success() {
                let err = String::from_utf8_lossy(&output.stderr);
                fail(format!("GIT_BUG_NEW_FAILED:{}", err.trim()));
            }
            let out_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let bug_id = out_str
                .lines()
                .next()
                .and_then(|line| {
                    line.split_whitespace()
                        .find(|part| part.chars().all(|c| c.is_ascii_hexdigit()) && part.len() >= 7)
                })
                .map(|s| s.to_string())
                .unwrap_or_else(|| out_str.clone());

            for l in &labels {
                let _ = Command::new(&git_bug)
                    .args(["bug", "label", "new", &bug_id, l])
                    .current_dir(&work_dir)
                    .output();
            }

            println!(
                "{{\"ok\":true,\"id\":\"{}\",\"title\":\"{}\",\"labels\":{:?},\"upstream\":{}}}",
                bug_id, title_str, labels, upstream
            );
        }
        "list" | "ls" => {
            let mut status: Option<String> = None;
            let mut labels: Vec<String> = Vec::new();
            let mut sort_by: Option<String> = None;
            let mut direction: Option<String> = None;
            let mut author: Option<String> = None;
            let mut participant: Option<String> = None;
            let mut query: Option<String> = None;
            let mut format_json = false;
            let mut upstream = false;

            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-s" | "--status" => {
                        status = Some(args.next().unwrap_or_else(|| fail("MISSING_STATUS_ARG")));
                    }
                    "-l" | "--label" => {
                        let l = args.next().unwrap_or_else(|| fail("MISSING_LABEL_ARG"));
                        for item in l.split(',') {
                            let trimmed = item.trim();
                            if !trimmed.is_empty() {
                                labels.push(trimmed.to_string());
                            }
                        }
                    }
                    "-b" | "--by" | "--sort" => {
                        sort_by = Some(args.next().unwrap_or_else(|| fail("MISSING_SORT_ARG")));
                    }
                    "-d" | "--direction" => {
                        direction =
                            Some(args.next().unwrap_or_else(|| fail("MISSING_DIRECTION_ARG")));
                    }
                    "-a" | "--author" => {
                        author = Some(args.next().unwrap_or_else(|| fail("MISSING_AUTHOR_ARG")));
                    }
                    "-p" | "--participant" => {
                        participant = Some(
                            args.next()
                                .unwrap_or_else(|| fail("MISSING_PARTICIPANT_ARG")),
                        );
                    }
                    "-q" | "--query" => {
                        query = Some(args.next().unwrap_or_else(|| fail("MISSING_QUERY_ARG")));
                    }
                    "-f" => {
                        let f = args.next().unwrap_or_else(|| fail("MISSING_FORMAT_ARG"));
                        if f == "json" {
                            format_json = true;
                        }
                    }
                    "--json" => {
                        format_json = true;
                    }
                    "--upstream" => {
                        upstream = true;
                    }
                    _ => fail(format!("UNKNOWN_BUG_LIST_OPTION:{}", arg)),
                }
            }

            let work_dir = select_bug_store(root, upstream).unwrap_or_else(|error| fail(error));

            let build_cmd = |dir: &Path| {
                let mut cmd = Command::new(&git_bug);
                cmd.arg("bug");
                if let Some(ref q) = query {
                    cmd.arg(q);
                }
                if let Some(ref s) = status {
                    cmd.args(["--status", s]);
                }
                for l in &labels {
                    cmd.args(["--label", l]);
                }
                if let Some(ref b) = sort_by {
                    cmd.args(["--by", b]);
                }
                if let Some(ref d) = direction {
                    cmd.args(["--direction", d]);
                }
                if let Some(ref a) = author {
                    cmd.args(["--author", a]);
                }
                if let Some(ref p) = participant {
                    cmd.args(["--participant", p]);
                }
                if format_json {
                    cmd.args(["-f", "json"]);
                }
                cmd.current_dir(dir);
                cmd
            };

            let output = run_git_bug_read(|| build_cmd(&work_dir).output(), format_json)
                .unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));

            if !output.status.success() {
                let err = String::from_utf8_lossy(&output.stderr);
                fail(format!("GIT_BUG_LIST_FAILED:{}", err.trim()));
            }
            let out_str = String::from_utf8_lossy(&output.stdout);
            print!("{}", out_str);
        }
        "show" => {
            let bug_id = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk bug show <id> [--json] [--upstream]"));
            let mut format_json = false;
            let mut upstream = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-f" => {
                        if args.next().as_deref() == Some("json") {
                            format_json = true;
                        }
                    }
                    "--json" => format_json = true,
                    "--upstream" => upstream = true,
                    _ => {}
                }
            }
            let work_dir = select_bug_store(root, upstream).unwrap_or_else(|error| fail(error));

            let run_show = |dir: &Path| {
                let mut cmd = Command::new(&git_bug);
                cmd.args(["bug", "show", &bug_id]);
                if format_json {
                    cmd.args(["-f", "json"]);
                }
                cmd.current_dir(dir);
                cmd.output()
            };

            let output = run_git_bug_read(|| run_show(&work_dir), format_json)
                .unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));

            if !output.status.success() {
                let err = String::from_utf8_lossy(&output.stderr);
                fail(format!("GIT_BUG_SHOW_FAILED:{}", err.trim()));
            }
            if format_json {
                let mut record: Value = serde_json::from_slice(&output.stdout)
                    .unwrap_or_else(|_| fail("GIT_BUG_SHOW_INVALID_JSON"));
                if let Some(close_event) = git_bug_close_event(&work_dir, &record) {
                    record["close_event"] = close_event;
                }
                println!("{}", serde_json::to_string_pretty(&record).unwrap());
            } else {
                print!("{}", String::from_utf8_lossy(&output.stdout));
            }
        }
        "comment" => {
            let bug_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk bug comment <id> [-m] <message> [--upstream]")
            });
            let mut msg: Option<String> = None;
            let mut upstream = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-m" | "--message" => {
                        msg = Some(
                            args.next()
                                .unwrap_or_else(|| fail("MISSING_COMMENT_MESSAGE")),
                        );
                    }
                    "--upstream" => {
                        upstream = true;
                    }
                    other => {
                        if msg.is_none() {
                            msg = Some(other.to_string());
                        }
                    }
                }
            }
            let message = msg.unwrap_or_else(|| {
                fail("USAGE: appsdk bug comment <id> [-m] <message> [--upstream]")
            });
            let work_dir = select_bug_store(root, upstream).unwrap_or_else(|error| fail(error));

            let run_comment = |dir: &Path| {
                ensure_identity(dir);
                let mut cmd = Command::new(&git_bug);
                cmd.args(["bug", "comment", "new", &bug_id, "-m", &message]);
                cmd.current_dir(dir);
                cmd.output()
            };

            let output =
                run_comment(&work_dir).unwrap_or_else(|_| fail("GIT_BUG_EXECUTION_FAILED"));
            if !output.status.success() {
                let err = String::from_utf8_lossy(&output.stderr);
                fail(format!("GIT_BUG_COMMENT_FAILED:{}", err.trim()));
            }
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        "close" => {
            let bug_id = args.next().unwrap_or_else(|| {
                fail(
                    "USAGE: appsdk bug close <id> [-m <solution>] [--receipt-id <id>] [--upstream]",
                )
            });
            let mut receipt_id: Option<String> = None;
            let mut solution: Option<String> = None;
            let mut upstream = false;

            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--receipt-id" => {
                        receipt_id = Some(
                            args.next()
                                .unwrap_or_else(|| fail("MISSING_RECEIPT_ID_ARG")),
                        );
                    }
                    "-m" | "--message" | "--solution" => {
                        solution =
                            Some(args.next().unwrap_or_else(|| fail("MISSING_SOLUTION_ARG")));
                    }
                    "--upstream" => {
                        upstream = true;
                    }
                    _ => fail(format!("UNKNOWN_BUG_CLOSE_OPTION:{}", arg)),
                }
            }
            let solution = solution.unwrap_or_else(|| {
                fail("USAGE: appsdk bug close <id> -m <solution> [--receipt-id <id>] [--upstream]")
            });
            let close_notes = {
                let mut notes = Vec::new();
                notes.push(format!("### Solution / Resolution\n{}", solution));
                if let Some(r_id) = receipt_id {
                    notes.push(format!("### Mainline Receipt\n{}", r_id));
                }
                notes
            };

            let target_dir = if upstream {
                resolve_upstream_repo().unwrap_or_else(|error| fail(error))
            } else {
                root.to_path_buf()
            };

            let run_close = |dir: &Path| -> Result<(), String> {
                ensure_identity(dir);
                if !close_notes.is_empty() {
                    let msg = close_notes.join("\n\n");
                    let mut cmd = Command::new(&git_bug);
                    cmd.args(["bug", "comment", "new", &bug_id, "-m", &msg]);
                    cmd.current_dir(dir);
                    let output = cmd
                        .output()
                        .map_err(|_| "GIT_BUG_EXECUTION_FAILED".to_string())?;
                    if !output.status.success() {
                        let err = String::from_utf8_lossy(&output.stderr);
                        return Err(format!("GIT_BUG_COMMENT_FAILED:{}", err.trim()));
                    }
                }

                let mut cmd = Command::new(&git_bug);
                cmd.args(["bug", "status", "close", &bug_id]);
                cmd.current_dir(dir);

                let output = cmd
                    .output()
                    .map_err(|_| "GIT_BUG_EXECUTION_FAILED".to_string())?;
                if !output.status.success() {
                    let err = String::from_utf8_lossy(&output.stderr);
                    return Err(format!("GIT_BUG_CLOSE_FAILED:{}", err.trim()));
                }
                Ok(())
            };

            match run_close(&target_dir) {
                Ok(_) => {
                    println!(
                        "{{\"ok\":true,\"bug_id\":\"{}\",\"status\":\"closed\"}}",
                        bug_id
                    );
                }
                Err(e) => {
                    fail(e);
                }
            }
        }
        "webui" => {
            let mut port: Option<String> = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-p" | "--port" => {
                        port = Some(args.next().unwrap_or_else(|| fail("MISSING_PORT_ARG")));
                    }
                    _ => fail(format!("UNKNOWN_WEBUI_OPTION:{}", arg)),
                }
            }
            let mut cmd = Command::new(&git_bug);
            cmd.arg("webui");
            if let Some(p) = port {
                cmd.args(["--port", &p]);
            }
            cmd.current_dir(root);
            println!("Launching git-bug webui for {} ...", root.display());
            let _ = cmd
                .status()
                .unwrap_or_else(|_| fail("GIT_BUG_WEBUI_FAILED"));
        }
        _ => fail(format!("UNKNOWN_BUG_SUBCOMMAND:{}", sub)),
    }
}

fn parse_duration_to_ms(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("EMPTY_DURATION".into());
    }
    if let Ok(ms) = s.parse::<u64>() {
        return Ok(ms);
    }
    let (num_part, unit) = s.split_at(s.len() - 1);
    let num: u64 = num_part
        .parse()
        .map_err(|_| format!("INVALID_DURATION_NUMBER:{}", num_part))?;
    let multiplier = match unit {
        "s" | "S" => Ok(1_000_u64),
        "m" | "M" => Ok(60_000_u64),
        "h" | "H" => Ok(3_600_000_u64),
        "d" | "D" => Ok(86_400_000_u64),
        _ => Err(format!("UNKNOWN_DURATION_UNIT:{}", unit)),
    }?;
    num.checked_mul(multiplier)
        .ok_or_else(|| format!("GOAL_DURATION_OVERFLOW:{}", s))
}

fn long_horizon_record(root: &Path) -> Result<Option<Value>, String> {
    let file = root.join(".appsdk-control/long-task-goal.json");
    let content = match fs::read_to_string(&file) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "GOAL_RECORD_READ_FAILED:{}:{}",
                file.display(),
                error
            ));
        }
    };
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|error| format!("GOAL_RECORD_JSON_INVALID:{}", error))
}

fn collab_status_all(root: &Path) -> Result<Value, String> {
    let mut command = Command::new("collab");
    command.args(["status", "--all"]).current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_READ_TIMEOUT)
        .map_err(|error| format!("COLLAB_STATUS_UNAVAILABLE:{}", error))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!(
            "COLLAB_STATUS_FAILED:exit={}{}",
            out.status.code().unwrap_or(1),
            if detail.is_empty() {
                String::new()
            } else {
                format!(":{}", detail)
            }
        ));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|error| format!("COLLAB_STATUS_JSON_INVALID:{}", error))
}

fn collab_master_status(root: &Path) -> Result<Value, String> {
    let mut command = Command::new("collab");
    command.args(["master", "status"]).current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_READ_TIMEOUT)
        .map_err(|error| format!("GOAL_OWNER_MASTER_STATUS_UNAVAILABLE:{}", error))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!(
            "GOAL_OWNER_MASTER_STATUS_FAILED:exit={}{}",
            out.status.code().unwrap_or(1),
            if detail.is_empty() {
                String::new()
            } else {
                format!(":{}", detail)
            }
        ));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|error| format!("GOAL_OWNER_MASTER_STATUS_JSON_INVALID:{}", error))
}

fn goal_record_subscription_id(record: &Value) -> Option<String> {
    record
        .get("subscription_id")
        .and_then(Value::as_str)
        .or_else(|| {
            record
                .get("collab_subscription")
                .and_then(|value| value.get("subscription_id"))
                .and_then(Value::as_str)
        })
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
}

fn goal_record_read(root: &Path) -> Result<Option<Value>, String> {
    long_horizon_record(root)
}

fn goal_record_write(root: &Path, record: &Value) -> Result<(), String> {
    let control_dir = root.join(".appsdk-control");
    fs::create_dir_all(&control_dir)
        .map_err(|error| format!("GOAL_CONTROL_DIR_CREATE_FAILED:{}", error))?;
    let file = control_dir.join("long-task-goal.json");
    let content = serde_json::to_string_pretty(record)
        .map_err(|error| format!("GOAL_RECORD_SERIALIZE_FAILED:{}", error))?
        + "\n";
    let staging = control_dir.join(format!(
        "long-task-goal.json.staging.{}.{}",
        std::process::id(),
        goal_now_ms()
    ));
    fs::write(&staging, content).map_err(|error| format!("GOAL_RECORD_WRITE_FAILED:{}", error))?;
    if let Err(error) = fs::rename(&staging, &file) {
        let _ = fs::remove_file(&staging);
        return Err(format!("GOAL_RECORD_WRITE_FAILED:{}", error));
    }
    Ok(())
}

fn goal_mark_recovery_required(record: &mut Value, error: String) {
    record["desired"] = Value::String("recovery_required".into());
    record["observed"] = Value::String("unknown".into());
    record["remote_state"] = Value::String("unknown".into());
    record["active"] = Value::Bool(false);
    record["error"] = Value::String(error);
    record["recovery"] = Value::String(
        "Restore Collab if needed, then rerun appsdk goal subscribe --goal <path.md> to rearm a fresh one-shot deadline; no automatic renewal is attempted".into(),
    );
    record["revision"] = Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
}

struct GoalLock {
    _file: fs::File,
    path: PathBuf,
}

#[cfg(unix)]
fn goal_try_advisory_lock(file: &fs::File) -> Result<(), String> {
    const LOCK_EX: c_int = 2;
    const LOCK_NB: c_int = 4;
    unsafe extern "C" {
        fn flock(fd: c_int, operation: c_int) -> c_int;
    }
    let result = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == ErrorKind::WouldBlock {
        Err("GOAL_LOCK_BUSY".into())
    } else {
        Err(format!("GOAL_LOCK_ADVISORY_FAILED:{}", error))
    }
}

#[cfg(not(unix))]
fn goal_try_advisory_lock(_file: &fs::File) -> Result<(), String> {
    Ok(())
}

fn goal_lock_metadata(path: &Path) -> Result<String, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("GOAL_LOCK_METADATA_READ_FAILED:{}", error))?;
    let pid = contents
        .split_whitespace()
        .find_map(|field| field.strip_prefix("pid="))
        .ok_or_else(|| "GOAL_LOCK_METADATA_INVALID:pid missing".to_string())?;
    pid.parse::<u32>()
        .map_err(|error| format!("GOAL_LOCK_METADATA_INVALID:{}", error))?;
    let owner = contents
        .split_whitespace()
        .find_map(|field| field.strip_prefix("owner="))
        .filter(|owner| !owner.trim().is_empty())
        .ok_or_else(|| "GOAL_LOCK_METADATA_INVALID:owner missing".to_string())?;
    Ok(format!("pid={} owner={}", pid, owner))
}

fn goal_lock_recovery_receipt(
    control_dir: &Path,
    quarantined: &Path,
    original: &str,
    reason: &str,
) -> Result<(), String> {
    let receipt = control_dir.join(format!(
        "long-task-goal.lock.recovery.{}.{}.json",
        std::process::id(),
        goal_now_ms()
    ));
    let payload = serde_json::json!({
        "schema_version": 1,
        "status": "recovered",
        "reason": reason,
        "original_metadata": original,
        "quarantined_path": quarantined.to_string_lossy(),
        "recovered_by_pid": std::process::id(),
        "recovered_at": chrono::Utc::now().to_rfc3339(),
        "recovery": "The advisory lock was released by its prior process; the old path was quarantined atomically before reacquisition."
    });
    fs::write(
        &receipt,
        serde_json::to_string_pretty(&payload)
            .map_err(|error| format!("GOAL_LOCK_RECOVERY_RECEIPT_SERIALIZE_FAILED:{}", error))?
            + "\n",
    )
    .map_err(|error| format!("GOAL_LOCK_RECOVERY_RECEIPT_WRITE_FAILED:{}", error))
}

impl GoalLock {
    fn acquire(root: &Path, owner: &str) -> Result<Self, String> {
        let control_dir = root.join(".appsdk-control");
        fs::create_dir_all(&control_dir)
            .map_err(|error| format!("GOAL_CONTROL_DIR_CREATE_FAILED:{}", error))?;
        let path = control_dir.join("long-task-goal.lock");
        for _ in 0..3 {
            let mut lock = match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(lock) => {
                    if let Err(error) = goal_try_advisory_lock(&lock) {
                        return Err(error);
                    }
                    lock
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    let existing = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                        .map_err(|open_error| format!("GOAL_LOCK_ACQUIRE_FAILED:{}", open_error))?;
                    if let Err(error) = goal_try_advisory_lock(&existing) {
                        if error == "GOAL_LOCK_BUSY" {
                            let metadata = fs::read_to_string(&path).unwrap_or_default();
                            return Err(if metadata.trim().is_empty() {
                                "GOAL_LOCK_BUSY: an active goal lifecycle operation holds the advisory lock; metadata is empty".into()
                            } else {
                                format!(
                                    "GOAL_LOCK_BUSY: an active goal lifecycle operation holds the advisory lock ({})",
                                    metadata.trim()
                                )
                            });
                        }
                        return Err(error);
                    }
                    let original = fs::read_to_string(&path).unwrap_or_default();
                    let quarantined = control_dir.join(format!(
                        "long-task-goal.lock.recovered.{}.{}",
                        std::process::id(),
                        goal_now_ms()
                    ));
                    match fs::rename(&path, &quarantined) {
                        Ok(()) => {
                            if let Err(receipt_error) = goal_lock_recovery_receipt(
                                &control_dir,
                                &quarantined,
                                &original,
                                if original.trim().is_empty() {
                                    "empty metadata"
                                } else if goal_lock_metadata(&quarantined).is_err() {
                                    "invalid or truncated metadata"
                                } else {
                                    "advisory lock released with stale metadata"
                                },
                            ) {
                                return Err(receipt_error);
                            }
                            continue;
                        }
                        Err(rename_error) if rename_error.kind() == ErrorKind::NotFound => {
                            continue;
                        }
                        Err(rename_error) => {
                            return Err(format!(
                                "GOAL_LOCK_STALE_RECOVERY_FAILED:{}",
                                rename_error
                            ));
                        }
                    }
                }
                Err(error) => return Err(format!("GOAL_LOCK_ACQUIRE_FAILED:{}", error)),
            };
            if let Err(error) = lock.set_len(0).and_then(|_| {
                writeln!(lock, "pid={} owner={}", std::process::id(), owner)?;
                lock.sync_all()
            }) {
                let _ = fs::remove_file(&path);
                return Err(format!("GOAL_LOCK_WRITE_FAILED:{}", error));
            }
            return Ok(Self { _file: lock, path });
        }
        Err("GOAL_LOCK_BUSY: stale lock changed during recovery".into())
    }
}

impl Drop for GoalLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn goal_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn join_goal_output_reader(reader: Option<thread::JoinHandle<Vec<u8>>>) {
    if let Some(reader) = reader {
        let _ = reader.join();
    }
}

fn detach_goal_output_readers(
    stdout_reader: Option<thread::JoinHandle<Vec<u8>>>,
    stderr_reader: Option<thread::JoinHandle<Vec<u8>>>,
) {
    // A killed command may have descendants holding inherited pipe writers;
    // join them off the timeout path after those writers close.
    let _ = thread::spawn(move || {
        join_goal_output_reader(stdout_reader);
        join_goal_output_reader(stderr_reader);
    });
}

fn join_goal_output_reader_until(
    reader: thread::JoinHandle<Vec<u8>>,
    deadline: Instant,
) -> Result<Vec<u8>, thread::JoinHandle<Vec<u8>>> {
    let reader = reader;
    while !reader.is_finished() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(reader);
        }
        thread::sleep(remaining.min(Duration::from_millis(10)));
    }
    Ok(reader.join().ok().unwrap_or_default())
}

fn drain_goal_output_readers(
    stdout_reader: Option<thread::JoinHandle<Vec<u8>>>,
    stderr_reader: Option<thread::JoinHandle<Vec<u8>>>,
    deadline: Instant,
) -> Result<(Vec<u8>, Vec<u8>), ()> {
    let stdout = match stdout_reader {
        Some(reader) => match join_goal_output_reader_until(reader, deadline) {
            Ok(stdout) => stdout,
            Err(reader) => {
                detach_goal_output_readers(Some(reader), stderr_reader);
                return Err(());
            }
        },
        None => Vec::new(),
    };
    let stderr = match stderr_reader {
        Some(reader) => match join_goal_output_reader_until(reader, deadline) {
            Ok(stderr) => stderr,
            Err(reader) => {
                detach_goal_output_readers(None, Some(reader));
                return Err(());
            }
        },
        None => Vec::new(),
    };
    Ok((stdout, stderr))
}

// Collab may queue a command behind an active daemon batch. Keep every goal
// lifecycle call within one declared 120-second batch budget.
const GOAL_COLLAB_READ_TIMEOUT: Duration = Duration::from_secs(120);
const GOAL_COLLAB_WRITE_TIMEOUT: Duration = Duration::from_secs(120);

fn run_goal_collab_command(mut command: Command, timeout: Duration) -> Result<Output, String> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("GOAL_COLLAB_COMMAND_UNAVAILABLE:{}", error))?;
    let stdout_reader = child.stdout.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut stdout = Vec::new();
            let _ = pipe.read_to_end(&mut stdout);
            stdout
        })
    });
    let stderr_reader = child.stderr.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut stderr = Vec::new();
            let _ = pipe.read_to_end(&mut stderr);
            stderr
        })
    });
    let started = Instant::now();
    let deadline = started + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return match drain_goal_output_readers(stdout_reader, stderr_reader, deadline) {
                    Ok((stdout, stderr)) => Ok(Output {
                        status,
                        stdout,
                        stderr,
                    }),
                    Err(()) => Err("GOAL_COLLAB_OUTPUT_DRAIN_TIMEOUT".into()),
                };
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                detach_goal_output_readers(stdout_reader, stderr_reader);
                return Err("GOAL_COLLAB_COMMAND_TIMEOUT".into());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                detach_goal_output_readers(stdout_reader, stderr_reader);
                return Err(format!("GOAL_COLLAB_COMMAND_WAIT_FAILED:{}", error));
            }
        }
    }
}

#[cfg(test)]
mod goal_collab_command_tests {
    use super::*;

    #[test]
    fn injected_timeout_returns_explicit_error_without_long_wait() {
        let started = Instant::now();
        let mut command = Command::new("/bin/sleep");
        command.arg("1");
        let result = run_goal_collab_command(command, Duration::from_millis(100));

        assert!(matches!(
            result,
            Err(error) if error == "GOAL_COLLAB_COMMAND_TIMEOUT"
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn injected_timeout_bounds_output_pipe_drain_without_false_success() {
        let started = Instant::now();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "/bin/sleep 5 & printf inherited-pipe"]);

        let result = run_goal_collab_command(command, Duration::from_millis(100));

        assert!(matches!(
            result,
            Err(error) if error == "GOAL_COLLAB_OUTPUT_DRAIN_TIMEOUT"
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}

fn verified_goal_master(root: &Path) -> Result<String, String> {
    let status = collab_status_all(root)?;
    let mut context_command = Command::new("collab");
    context_command.args(["context"]).current_dir(root);
    let context_output = run_goal_collab_command(context_command, GOAL_COLLAB_READ_TIMEOUT)
        .map_err(|error| format!("GOAL_OWNER_CONTEXT_UNAVAILABLE:{}", error))?;
    if !context_output.status.success() {
        return Err(format!(
            "GOAL_OWNER_CONTEXT_FAILED:exit={}",
            context_output.status.code().unwrap_or(1)
        ));
    }
    let context: Value = serde_json::from_slice(&context_output.stdout)
        .map_err(|error| format!("GOAL_OWNER_CONTEXT_JSON_INVALID:{}", error))?;
    let owner = context["identity"]["worker_id"]
        .as_str()
        .filter(|owner| !owner.trim().is_empty())
        .ok_or_else(|| "GOAL_OWNER_IDENTITY_MISSING".to_string())?;

    let worker = status["workers"]
        .as_array()
        .and_then(|workers| {
            workers
                .iter()
                .find(|worker| worker["id"].as_str() == Some(owner))
        })
        .ok_or_else(|| "GOAL_OWNER_WORKER_MISSING".to_string())?;
    if worker["endpoint_live"].as_bool() != Some(true) {
        return Err("GOAL_OWNER_NOT_LIVE:verified Collab owner endpoint is not live".into());
    }
    if worker["identity_valid"].as_bool() != Some(true) {
        return Err("GOAL_OWNER_IDENTITY_INVALID:verified Collab owner identity is invalid".into());
    }

    let master_status = collab_master_status(root)?;
    let master = master_status
        .get("master")
        .filter(|value| value.is_object())
        .ok_or_else(|| "GOAL_OWNER_MASTER_IDENTITY_MISSING".to_string())?;
    let master_owner = master["worker_id"]
        .as_str()
        .filter(|worker_id| !worker_id.trim().is_empty())
        .ok_or_else(|| "GOAL_OWNER_MASTER_IDENTITY_MISSING".to_string())?;
    if master["endpoint_live"].as_bool() != Some(true) {
        return Err("GOAL_OWNER_MASTER_NOT_LIVE".into());
    }
    let master_pane = master["pane"]
        .as_str()
        .filter(|pane| !pane.trim().is_empty())
        .ok_or_else(|| "GOAL_OWNER_MASTER_PANE_MISSING".to_string())?;
    let context_pane = context["identity"]["pane"]
        .as_str()
        .filter(|pane| !pane.trim().is_empty())
        .ok_or_else(|| "GOAL_OWNER_CONTEXT_PANE_MISSING".to_string())?;
    if master_owner != owner {
        return Err(format!(
            "GOAL_OWNER_IDENTITY_MISMATCH:context={} master={}",
            owner, master_owner
        ));
    }
    if master_pane != context_pane {
        return Err(format!(
            "GOAL_OWNER_PANE_MISMATCH:context={} master={}",
            context_pane, master_pane
        ));
    }
    match worker["suspected_offline"].as_bool() {
        Some(false) => {}
        Some(true) if master_owner == owner && master["endpoint_live"].as_bool() == Some(true) => {}
        _ => {
            return Err(
                "GOAL_OWNER_SUSPECTED_OFFLINE:verified Collab owner is suspected offline".into(),
            );
        }
    }
    Ok(owner.to_string())
}

fn parse_goal_subscription_response(stdout: &[u8]) -> Result<(Value, String), String> {
    let response: Value = serde_json::from_slice(stdout)
        .map_err(|error| format!("COLLAB_SUBSCRIBE_RESPONSE_INVALID:{}", error))?;
    let subscription_id = {
        let subscription = response.get("subscription").unwrap_or(&response);
        subscription
            .get("subscription_id")
            .and_then(Value::as_str)
            .or_else(|| subscription.get("id").and_then(Value::as_str))
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| "COLLAB_SUBSCRIBE_RESPONSE_MISSING_ID".to_string())?
            .to_string()
    };
    Ok((response, subscription_id))
}

fn parse_goal_cancel_response(stdout: &[u8], expected_id: &str) -> Result<Value, String> {
    let response: Value = serde_json::from_slice(stdout)
        .map_err(|error| format!("GOAL_CANCEL_RESPONSE_INVALID:{}", error))?;
    let response_id = response
        .get("subscription_id")
        .and_then(Value::as_str)
        .or_else(|| {
            response
                .get("subscription")
                .and_then(|subscription| subscription.get("subscription_id"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            response
                .get("subscription")
                .and_then(|subscription| subscription.get("id"))
                .and_then(Value::as_str)
        });
    if response_id != Some(expected_id) {
        return Err("GOAL_CANCEL_RESPONSE_ID_MISMATCH".to_string());
    }
    if response.get("status").and_then(Value::as_str) != Some("cancelled") {
        return Err("GOAL_CANCEL_RESPONSE_STATUS_INVALID".to_string());
    }
    Ok(response)
}

fn goal_cancel_subscription(root: &Path, subscription_id: &str) -> Result<Value, String> {
    let mut command = Command::new("collab");
    command
        .args(["notify", "unsubscribe", subscription_id])
        .current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_WRITE_TIMEOUT).map_err(|error| {
        if error == "GOAL_COLLAB_COMMAND_TIMEOUT" {
            error
        } else {
            format!("GOAL_CANCEL_COLLAB_FAILED:{}", error)
        }
    })?;
    if !out.status.success() {
        return Err(format!(
            "GOAL_CANCEL_COLLAB_FAILED:exit={}{}",
            out.status.code().unwrap_or(1),
            if out.stderr.is_empty() {
                String::new()
            } else {
                format!(":{}", String::from_utf8_lossy(&out.stderr).trim())
            }
        ));
    }
    parse_goal_cancel_response(&out.stdout, subscription_id)
}

fn goal_subscription_status(root: &Path, subscription_id: &str) -> Result<(String, Value), String> {
    let mut command = Command::new("collab");
    command.args(["notify", "status"]).current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_READ_TIMEOUT)
        .map_err(|error| format!("GOAL_STATUS_COLLAB_UNAVAILABLE:{}", error))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!(
            "GOAL_STATUS_COLLAB_FAILED:exit={}{}",
            out.status.code().unwrap_or(1),
            if detail.is_empty() {
                String::new()
            } else {
                format!(":{}", detail)
            }
        ));
    }
    let response: Value = serde_json::from_slice(&out.stdout)
        .map_err(|error| format!("GOAL_STATUS_RESPONSE_INVALID:{}", error))?;
    let subscriptions = response
        .get("subscriptions")
        .and_then(Value::as_array)
        .ok_or_else(|| "GOAL_STATUS_RESPONSE_MISSING_SUBSCRIPTIONS".to_string())?;
    let subscription = subscriptions.iter().find(|subscription| {
        subscription
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| subscription.get("subscription_id").and_then(Value::as_str))
            == Some(subscription_id)
    });
    let Some(subscription) = subscription else {
        return Err(format!(
            "GOAL_STATUS_SUBSCRIPTION_NOT_FOUND:{}",
            subscription_id
        ));
    };
    let remote_status = subscription
        .get("status")
        .and_then(Value::as_str)
        .filter(|status| !status.trim().is_empty())
        .ok_or_else(|| "GOAL_STATUS_SUBSCRIPTION_STATUS_MISSING".to_string())?;
    Ok((remote_status.to_string(), subscription.clone()))
}

fn goal_subscription_by_subject(
    root: &Path,
    subject: &str,
) -> Result<Option<(String, String, Value)>, String> {
    let mut command = Command::new("collab");
    command.args(["notify", "status"]).current_dir(root);
    let out = run_goal_collab_command(command, GOAL_COLLAB_READ_TIMEOUT)
        .map_err(|error| format!("GOAL_RECONCILE_COLLAB_UNAVAILABLE:{}", error))?;
    if !out.status.success() {
        return Err(format!(
            "GOAL_RECONCILE_COLLAB_FAILED:exit={}",
            out.status.code().unwrap_or(1)
        ));
    }
    let response: Value = serde_json::from_slice(&out.stdout)
        .map_err(|error| format!("GOAL_RECONCILE_RESPONSE_INVALID:{}", error))?;
    let subscriptions = response
        .get("subscriptions")
        .and_then(Value::as_array)
        .ok_or_else(|| "GOAL_RECONCILE_RESPONSE_MISSING_SUBSCRIPTIONS".to_string())?;
    let matches: Vec<(String, String, Value)> = subscriptions
        .iter()
        .filter_map(|subscription| {
            let id = subscription
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| subscription.get("subscription_id").and_then(Value::as_str))
                .filter(|id| !id.trim().is_empty())?;
            let status = subscription.get("status").and_then(Value::as_str)?;
            let event = subscription.get("event").and_then(Value::as_str)?;
            let remote_subject = subscription.get("subject").and_then(Value::as_str)?;
            (event == "deadline" && remote_subject == subject && status == "armed")
                .then(|| (id.to_string(), status.to_string(), subscription.clone()))
        })
        .collect();
    if matches.len() > 1 {
        return Err(format!(
            "GOAL_RECONCILE_SUBJECT_AMBIGUOUS:{} armed deadline subscriptions match",
            matches.len()
        ));
    }
    Ok(matches.into_iter().next())
}

fn goal_subject_candidates(record: Option<&Value>, canonical_subject: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut push_unique = |subject: String| {
        if !subject.trim().is_empty() && !candidates.iter().any(|item| item == &subject) {
            candidates.push(subject);
        }
    };
    if let Some(record) = record {
        if let Some(subject) = record["subject"]
            .as_str()
            .filter(|subject| !subject.trim().is_empty())
        {
            // The retained subject is the compatibility anchor. Query it first
            // so a legacy basename subscription is never silently abandoned.
            push_unique(subject.to_string());
        }
        if let Some(goal_path) = record["goal_path"].as_str() {
            if let Some(basename) = Path::new(goal_path)
                .file_name()
                .and_then(|name| name.to_str())
            {
                push_unique(format!("goal:{}", basename));
            }
        }
    }
    push_unique(canonical_subject.to_string());
    candidates
}

fn goal_subscription_by_subject_candidates(
    root: &Path,
    record: Option<&Value>,
    canonical_subject: &str,
) -> Result<Option<(String, String, Value, String)>, String> {
    let mut matches: Vec<(String, String, Value, String)> = Vec::new();
    for subject in goal_subject_candidates(record, canonical_subject) {
        if let Some((id, status, remote_record)) = goal_subscription_by_subject(root, &subject)? {
            if !matches
                .iter()
                .any(|(matched_id, _, _, _)| matched_id == &id)
            {
                matches.push((id, status, remote_record, subject));
            }
        }
    }
    if matches.len() > 1 {
        return Err(format!(
            "GOAL_RECONCILE_SUBJECT_AMBIGUOUS:{} distinct armed deadline subscriptions match retained or canonical subjects",
            matches.len()
        ));
    }
    Ok(matches.into_iter().next())
}

fn goal_fail(format_json: bool, error: &str, record: Option<&Value>) -> ! {
    eprintln!("{}", error);
    if format_json {
        let mut payload = record.cloned().unwrap_or_else(|| {
            serde_json::json!({
                "active": false,
                "desired": "unknown",
                "observed": "unknown"
            })
        });
        payload["ok"] = Value::Bool(false);
        payload["error"] = Value::String(error.to_string());
        payload["recovery"] = Value::String(
            "inspect the retained goal record and retry after restoring Collab or local storage"
                .into(),
        );
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
    }
    std::process::exit(1);
}

fn open_bugs_json(root: &Path) -> Result<Vec<Value>, String> {
    let git_bug = match locate_git_bug_binary() {
        Ok(path) => path,
        Err(err) => return Err(err),
    };
    let read = |dir: &Path| {
        Command::new(&git_bug)
            .args(["bug", "--status", "open", "-f", "json"])
            .current_dir(dir)
            .output()
    };
    let out = run_git_bug_read(|| read(root), true)
        .map_err(|err| format!("GIT_BUG_EXECUTION_FAILED:{}", err))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!(
                "GIT_BUG_OPEN_READ_FAILED: exit={}",
                out.status.code().unwrap_or(-1)
            )
        } else {
            format!("GIT_BUG_OPEN_READ_FAILED:{}", err)
        });
    }
    let mut bugs: Vec<Value> = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("GIT_BUG_OPEN_JSON_INVALID:{}", e))?;
    let rank = |bug: &Value| -> u8 {
        let labels = bug["labels"].as_array().cloned().unwrap_or_default();
        for (priority, score) in [
            ("P0", 0u8),
            ("p0", 0),
            ("P1", 1),
            ("p1", 1),
            ("P2", 2),
            ("p2", 2),
        ] {
            if labels.iter().any(|l| l.as_str() == Some(priority)) {
                return score;
            }
        }
        3
    };
    bugs.sort_by_key(rank);
    Ok(bugs)
}

/// Pull the first meaningful prose out of the goal document so one read shows
/// what the project is for, without shipping the whole file into a wake.
fn goal_objective_excerpt(goal_path: &Path, max_lines: usize, max_chars: usize) -> String {
    let content = match fs::read_to_string(goal_path) {
        Ok(text) => text,
        Err(err) => return format!("(无法读取目标文档: {})", err),
    };

    let mut lines = content.lines().peekable();
    if lines.peek() == Some(&"---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
        }
    }

    let mut picked: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || trimmed.is_empty() {
            continue;
        }
        picked.push(trimmed);
        if picked.len() >= max_lines {
            break;
        }
    }

    if picked.is_empty() {
        return "(目标文档为空)".to_string();
    }

    let mut excerpt = picked.join("\n");
    if excerpt.chars().count() > max_chars {
        excerpt = excerpt.chars().take(max_chars).collect::<String>() + " …";
    }
    excerpt
}

fn handle_longhorizon_command<I>(root: &Path, mut args: I)
where
    I: Iterator<Item = String>,
{
    let sub = args.next().unwrap_or_else(|| "show".to_string());
    match sub.as_str() {
        "show" | "brief" => {}
        "--json" => {
            longhorizon_show(root, true);
            return;
        }
        other => fail(format!(
            "UNKNOWN_LONGHORIZON_SUBCOMMAND:{} (USAGE: appsdk longhorizon show [--json])",
            other
        )),
    }

    let mut format_json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => format_json = true,
            other => fail(format!("UNKNOWN_LONGHORIZON_SHOW_OPTION:{}", other)),
        }
    }
    longhorizon_show(root, format_json);
}

fn longhorizon_show(root: &Path, format_json: bool) {
    let (record, record_error) = match long_horizon_record(root) {
        Ok(record) => (record, None),
        Err(error) => (None, Some(error)),
    };
    let (status, collab_status_error) = match collab_status_all(root) {
        Ok(status) => (Some(status), None),
        Err(error) => (None, Some(error)),
    };
    let (bugs, open_bugs_error) = match open_bugs_json(root) {
        Ok(bugs) => (bugs, None),
        Err(err) => (Vec::new(), Some(err)),
    };
    let role = execution_role(root, &status);
    let role_label = role.label();
    let charter = role.charter();
    let fleet_rules = role.fleet_rules();

    let goal_path = record
        .as_ref()
        .and_then(|r| r["goal_path"].as_str())
        .map(PathBuf::from);
    let objective = goal_path
        .as_ref()
        .map(|p| goal_objective_excerpt(p, 24, 1200))
        .unwrap_or_else(|| {
            "(未注册长程目标，先运行 appsdk goal subscribe --goal <path.md>)".to_string()
        });

    let empty = Vec::new();
    let tasks = status
        .as_ref()
        .and_then(|s| s["tasks"].as_array())
        .unwrap_or(&empty);
    let workers = status
        .as_ref()
        .and_then(|s| s["workers"].as_array())
        .unwrap_or(&empty);

    let is_blocked = |task: &Value| {
        matches!(
            task["status"].as_str().unwrap_or(""),
            "blocked" | "waiting" | "resource-waiting"
        )
    };
    let blocked_tasks: Vec<&Value> = tasks.iter().filter(|t| is_blocked(t)).collect();
    let active_tasks: Vec<&Value> = tasks.iter().filter(|t| !is_blocked(t)).collect();

    // Spare capacity means a live pane holding no task. A worker whose pane is
    // lost still owns its task, so it is an intervention item, not capacity.
    let is_idle = |worker: &Value| {
        worker["active_task"].is_null()
            && worker["endpoint_live"].as_bool().unwrap_or(false)
            && worker["identity_valid"].as_bool().unwrap_or(false)
            && !worker["suspected_offline"].as_bool().unwrap_or(false)
    };
    let needs_intervention = |worker: &Value| {
        !worker["identity_valid"].as_bool().unwrap_or(false)
            || !worker["endpoint_live"].as_bool().unwrap_or(false)
            || worker["suspected_offline"].as_bool().unwrap_or(false)
    };
    let idle_workers: Vec<&Value> = workers.iter().filter(|w| is_idle(w)).collect();
    let broken_workers: Vec<&Value> = workers.iter().filter(|w| needs_intervention(w)).collect();

    if format_json {
        let payload = serde_json::json!({
            "role": role.role(),
            "role_label": role_label,
            "charter": charter,
            "fleet_rules": fleet_rules,
            "notification_rules": POLICY.notification_rules(),
            "goal": {
                "registered": record.is_some(),
                "record_error": record_error.as_deref(),
                "path": goal_path.as_ref().map(|p| p.to_string_lossy().to_string()),
                "interval": record.as_ref().and_then(|r| r["interval"].as_str()),
                "active": record.as_ref().and_then(|r| r["active"].as_bool()),
                "registered_at": record.as_ref().and_then(|r| r["registered_at"].as_str()),
                "objective": objective,
            },
            "assigned": active_tasks,
            "blocked": blocked_tasks,
            "idle_workers": idle_workers,
            "workers_needing_intervention": broken_workers,
            "open_bugs": bugs,
            "open_bugs_error": open_bugs_error,
            "collab_reachable": status.is_some(),
            "collab_status_error": collab_status_error.as_deref(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
        return;
    }

    println!("{}", "=".repeat(80));
    println!("LONG-HORIZON {} BRIEFING", role_label);
    println!("{}", "=".repeat(80));

    println!("\n## 0. 你的角色\n\n{}", charter);
    if !fleet_rules.is_empty() {
        println!("\n{}", fleet_rules);
    }
    println!("\n{}", POLICY.notification_rules());

    println!("\n## 1. 长程目标\n");
    if let Some(error) = record_error.as_deref() {
        println!(
            "- 目标状态未知，记录读取失败: {}。请恢复记录后重试。",
            error
        );
    } else {
        match (&record, &goal_path) {
        (Some(rec), Some(path)) => {
            println!("- 目标文档: {}", path.display());
            println!(
                "- 唤醒方式: 一次性 deadline，延迟 {} | 活跃: {} | 注册于: {}",
                rec["interval"].as_str().unwrap_or("unknown"),
                rec["active"].as_bool().unwrap_or(false),
                rec["registered_at"].as_str().unwrap_or("unknown")
            );
        }
        _ => println!("- 未注册长程目标。先运行 `appsdk goal subscribe --goal <path.md> --interval <period>`。"),
        }
    }
    println!("\n目标摘要:\n{}", objective);

    println!("\n## 2. 工作分配\n");
    if let Some(error) = collab_status_error {
        println!(
            "- collab 状态未知，无法读取任务与 worker 状态: {}。先恢复 Collab 再重试。",
            error
        );
    }
    println!("已分配任务 ({}):", active_tasks.len());
    if active_tasks.is_empty() {
        println!("- 无");
    }
    for task in &active_tasks {
        println!(
            "- {} [{}] owner={} next={}",
            task["id"].as_str().unwrap_or("?"),
            task["status"].as_str().unwrap_or("?"),
            task["owner"].as_str().unwrap_or("?"),
            task["next_step"].as_str().unwrap_or("(未记录)")
        );
    }

    println!("\n空闲产能 ({}):", idle_workers.len());
    if idle_workers.is_empty() {
        println!("- 无空闲 worker");
    }
    for worker in &idle_workers {
        println!(
            "- {} agent_state={}",
            worker["id"].as_str().unwrap_or("?"),
            worker["agent_state"].as_str().unwrap_or("?")
        );
    }

    println!("\n## 3. 阻塞与缺陷\n");
    println!("Blocked / 等待中的任务 ({}):", blocked_tasks.len());
    if blocked_tasks.is_empty() {
        println!("- 无");
    }
    for task in &blocked_tasks {
        println!(
            "- {} [{}] owner={} next={}",
            task["id"].as_str().unwrap_or("?"),
            task["status"].as_str().unwrap_or("?"),
            task["owner"].as_str().unwrap_or("?"),
            task["next_step"].as_str().unwrap_or("(未记录)")
        );
    }

    println!("\n需要介入的 worker ({}):", broken_workers.len());
    if broken_workers.is_empty() {
        println!("- 无");
    }
    for worker in &broken_workers {
        println!(
            "- {} status={} diagnostic={}",
            worker["id"].as_str().unwrap_or("?"),
            worker["status"].as_str().unwrap_or("?"),
            worker["diagnostic"].as_str().unwrap_or("(无)")
        );
    }

    if let Some(err) = open_bugs_error {
        println!("\n开放缺陷读取失败: {}", err);
    } else {
        println!("\n开放缺陷 ({}，P0 优先):", bugs.len());
        if bugs.is_empty() {
            println!("- 无");
        }
        for bug in bugs.iter().take(10) {
            let labels: Vec<&str> = bug["labels"]
                .as_array()
                .map(|arr| arr.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            println!(
                "- {} [{}] {}",
                bug["human_id"].as_str().unwrap_or("?"),
                labels.join(","),
                bug["title"].as_str().unwrap_or("?")
            );
        }
    }

    println!("\n## 4. 本轮下一步\n");
    match role {
        ExecutionRole::Master => {
            println!("从以下三者中选一个并立即执行，不要以 ACK 或\"已读\"结束本轮：");
            println!("1. 派发 ready 工作给空闲 worker（优先消除空闲产能）；");
            println!("2. 解决一个 blocker 或介入一个失联 worker；");
            println!("3. 用证据宣告某个阶段完成，并推动 verify / merge / close worktree。");
            println!("\n若确为外部门禁（需人类批准的不可逆操作、发布、成本、新范围）：");
            println!("  collab master wake hold --reason \"<门禁与解除条件>\" --ttl-seconds <n>");
        }
        ExecutionRole::Worker => {
            println!(
                "继续当前已拥有的任务；运行 `collab context` 查看自己的 task/scope 后执行下一步。"
            );
            println!("不在任务范围内不要尝试全局调度或关闭其他 worker。");
        }
        ExecutionRole::ManagedSubagent => {
            println!("回到 parent 分配的 assignment；完成后向 parent/master 返回证据，不进入全局 backlog。");
        }
        ExecutionRole::Unknown => {
            println!("身份未验证；先运行 `collab context` 确认当前 pane/peer，恢复绑定后再做正常任务动作。");
            println!("当前会话不获得 master 权力，不派单、不关闭其他 peer。");
        }
    }
}

fn handle_goal_command<I>(root: &Path, mut args: I)
where
    I: Iterator<Item = String>,
{
    let sub = args
        .next()
        .unwrap_or_else(|| fail("USAGE: appsdk goal <subscribe|status|cancel|prompt> [options]"));

    match sub.as_str() {
        "subscribe" | "register" => {
            let mut goal_file: Option<String> = None;
            let mut interval_str = "10m".to_string();
            let mut repeat_count: u32 = 100;
            let mut ttl_seconds: u64 = 604800;
            let mut format_json = false;

            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-g" | "--goal" => {
                        goal_file = Some(args.next().unwrap_or_else(|| fail("MISSING_GOAL_ARG")));
                    }
                    "-i" | "--interval" | "--every" | "--period" => {
                        interval_str = args.next().unwrap_or_else(|| fail("MISSING_INTERVAL_ARG"));
                    }
                    "-r" | "--repeat" | "--repeat-count" => {
                        let r = args.next().unwrap_or_else(|| fail("MISSING_REPEAT_ARG"));
                        repeat_count = r.parse::<u32>().unwrap_or_else(|error| {
                            fail(format!(
                                "GOAL_REPEAT_COUNT_INVALID: '{}' is not an integer: {}",
                                r, error
                            ))
                        });
                    }
                    "--ttl" | "--ttl-seconds" => {
                        let t = args.next().unwrap_or_else(|| fail("MISSING_TTL_ARG"));
                        ttl_seconds = t.parse::<u64>().unwrap_or_else(|error| {
                            fail(format!(
                                "GOAL_TTL_INVALID: '{}' is not an unsigned integer: {}",
                                t, error
                            ))
                        });
                    }
                    "--json" => format_json = true,
                    _ => fail(format!("UNKNOWN_GOAL_SUBSCRIBE_OPTION:{}", arg)),
                }
            }

            let raw_goal = goal_file.unwrap_or_else(|| {
                fail("USAGE: appsdk goal subscribe --goal <path.md> [--interval <duration>]")
            });
            if !(1..=100).contains(&repeat_count) {
                fail("GOAL_REPEAT_COUNT_INVALID: --repeat must be from 1 through 100");
            }
            if ttl_seconds == 0 {
                fail("GOAL_TTL_INVALID: '0' must be greater than zero");
            }
            if !raw_goal.to_lowercase().ends_with(".md") {
                fail(format!(
                    "GOAL_PATH_MUST_BE_MD_FILE: '{}' is not a markdown file (.md)",
                    raw_goal
                ));
            }

            let goal_path = if Path::new(&raw_goal).is_absolute() {
                PathBuf::from(&raw_goal)
            } else {
                root.join(&raw_goal)
            };

            if !goal_path.exists() || !goal_path.is_file() {
                fail(format!(
                    "GOAL_FILE_NOT_FOUND: '{}' does not exist or is not a file",
                    goal_path.display()
                ));
            }

            let every_ms = parse_duration_to_ms(&interval_str).unwrap_or_else(|e| fail(e));
            let master_prompt = generate_long_horizon_master_prompt(&goal_path, &interval_str);
            let canonical_goal_path = goal_path
                .canonicalize()
                .unwrap_or_else(|_| goal_path.clone());
            let goal_id = sha256(&canonical_goal_path.to_string_lossy());
            let goal_revision = fs::read(&goal_path)
                .map(|content| sha256(&String::from_utf8_lossy(&content)))
                .unwrap_or_else(|error| {
                    goal_fail(
                        format_json,
                        &format!("GOAL_FILE_READ_FAILED:{}", error),
                        None,
                    )
                });

            let owner = match verified_goal_master(root) {
                Ok(owner) => owner,
                Err(error) => goal_fail(format_json, &error, None),
            };
            let _goal_lock = match GoalLock::acquire(root, &owner) {
                Ok(lock) => lock,
                Err(error) => goal_fail(format_json, &error, None),
            };
            let existing = match goal_record_read(root) {
                Ok(existing) => existing,
                Err(error) => {
                    drop(_goal_lock);
                    goal_fail(format_json, &error, None)
                }
            };
            let goal_subject = format!("goal:{}", goal_id);
            if let Some(existing) = existing.as_ref() {
                if existing["desired"].as_str() == Some("cancel_pending") {
                    match goal_subscription_by_subject_candidates(
                        root,
                        Some(existing),
                        &goal_subject,
                    ) {
                        Ok(Some((subscription_id, _, _, matched_subject))) => {
                            let error = format!(
                                "GOAL_CANCEL_PENDING_RECONCILIATION_REQUIRED: armed subscription {} remains under subject {}; rerun goal cancel before subscribing",
                                subscription_id, matched_subject
                            );
                            drop(_goal_lock);
                            goal_fail(format_json, &error, Some(existing));
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let error =
                                format!("GOAL_CANCEL_PENDING_RECONCILIATION_FAILED:{}", error);
                            drop(_goal_lock);
                            goal_fail(format_json, &error, Some(existing));
                        }
                    }
                }
                if matches!(
                    existing["desired"].as_str(),
                    Some("subscribed" | "recovery_required")
                ) {
                    let recovering = existing["desired"].as_str() == Some("recovery_required");
                    match goal_subscription_by_subject_candidates(
                        root,
                        Some(existing),
                        &goal_subject,
                    ) {
                        Ok(Some((
                            subscription_id,
                            remote_status,
                            remote_record,
                            matched_subject,
                        ))) => {
                            if existing["goal_id"].as_str() == Some(goal_id.as_str()) {
                                let mut response = existing.clone();
                                let retained_subject =
                                    existing["subject"].as_str().map(str::to_owned);
                                response["subscription_id"] = Value::String(subscription_id);
                                response["collab_subscription"] = remote_record;
                                response["remote_state"] = Value::String(remote_status);
                                response["desired"] = Value::String("subscribed".into());
                                response["observed"] = Value::String("subscribed".into());
                                response["active"] = Value::Bool(true);
                                response["error"] = Value::Null;
                                response["revision"] = Value::Number(
                                    (existing["revision"].as_u64().unwrap_or(0) + 1).into(),
                                );
                                response["idempotent"] = Value::Bool(true);
                                response["master_prompt"] = Value::String(master_prompt);
                                if recovering {
                                    response["recovered_at"] =
                                        Value::String(chrono::Utc::now().to_rfc3339());
                                }
                                if retained_subject.as_deref() != Some(matched_subject.as_str()) {
                                    response["subject_migration"] = serde_json::json!({
                                        "from": retained_subject,
                                        "to": matched_subject,
                                        "status": "canonical_subject_migrated",
                                        "migrated_at": chrono::Utc::now().to_rfc3339()
                                    });
                                    response["subject"] = Value::String(matched_subject);
                                }
                                if let Err(error) = goal_record_write(root, &response) {
                                    drop(_goal_lock);
                                    goal_fail(format_json, &error, Some(&response));
                                }
                                if format_json {
                                    println!(
                                        "{}",
                                        serde_json::to_string_pretty(&response).unwrap()
                                    );
                                } else {
                                    println!("Long-horizon goal already registered:");
                                    println!("- Goal file: {}", canonical_goal_path.display());
                                    println!("- Existing Collab subscription retained");
                                }
                                return;
                            }
                            drop(_goal_lock);
                            goal_fail(
                                format_json,
                                "GOAL_ALREADY_SUBSCRIBED: cancel the existing goal before subscribing another",
                                Some(existing),
                            );
                        }
                        Ok(None) if recovering => {}
                        Ok(None) => {
                            let mut recovery = existing.clone();
                            goal_mark_recovery_required(
                                &mut recovery,
                                "GOAL_EXISTING_SUBSCRIPTION_NOT_RECONCILED: no armed deadline subscription matches the retained or canonical subject".into(),
                            );
                            if let Err(error) = goal_record_write(root, &recovery) {
                                drop(_goal_lock);
                                goal_fail(format_json, &error, Some(&recovery));
                            }
                            drop(_goal_lock);
                            goal_fail(
                                format_json,
                                recovery["error"].as_str().unwrap(),
                                Some(&recovery),
                            );
                        }
                        Err(error) => {
                            let mut recovery = existing.clone();
                            goal_mark_recovery_required(&mut recovery, error.clone());
                            if let Err(write_error) = goal_record_write(root, &recovery) {
                                drop(_goal_lock);
                                goal_fail(format_json, &write_error, Some(&recovery));
                            }
                            drop(_goal_lock);
                            goal_fail(format_json, &error, Some(&recovery));
                        }
                    }
                }
            }

            let now_ms = goal_now_ms();
            let trigger_ms = now_ms.saturating_add(every_ms.min(i64::MAX as u64) as i64);
            let mut record = serde_json::json!({
                "schema_version": 1,
                "goal_id": goal_id,
                "goal_revision": goal_revision,
                "revision": 1,
                "desired": "subscribed",
                "observed": "pending",
                "goal_path": canonical_goal_path.to_string_lossy(),
                "interval": interval_str,
                "every_ms": every_ms,
                "trigger_ms": trigger_ms,
                "repeat_count": 1,
                "requested_repeat_count": repeat_count,
                "schedule": "one-shot",
                "local_schedule": "periodic-rearm-intent",
                "rearm_interval_ms": every_ms,
                "ttl_seconds": ttl_seconds,
                "owner": owner,
                "subject": goal_subject,
                "collab_subscribed": false,
                "collab_subscription": Value::Null,
                "subscription_id": Value::Null,
                "remote_state": "pending",
                "error": Value::Null,
                "registered_at": chrono::Utc::now().to_rfc3339(),
                "active": false,
                "recovery": "When the one-shot deadline is consumed, expires, or Collab restarts, rerun appsdk goal subscribe with this goal to create a fresh one-shot deadline; renewal is explicit and is not automatic."
            });
            if let Some(previous) = existing
                .as_ref()
                .filter(|previous| previous["desired"].as_str() == Some("recovery_required"))
            {
                let mut history = previous["recovery_history"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                history.push(serde_json::json!({
                    "previous_record": previous,
                    "recovered_at": chrono::Utc::now().to_rfc3339()
                }));
                record["recovery_history"] = Value::Array(history);
            }
            if let Err(error) = goal_record_write(root, &record) {
                record["error"] = Value::String(error.clone());
                drop(_goal_lock);
                goal_fail(format_json, &error, Some(&record));
            }
            let mut collab_command = Command::new("collab");
            collab_command
                .args([
                    "notify",
                    "subscribe",
                    "--event",
                    "deadline",
                    "--at-ms",
                    &trigger_ms.to_string(),
                    "--ttl-seconds",
                    &ttl_seconds.to_string(),
                    "--subject",
                    &goal_subject,
                ])
                .current_dir(root);
            let collab_sub = run_goal_collab_command(collab_command, GOAL_COLLAB_WRITE_TIMEOUT);

            let (collab_subscribed, sub_details, subscription_id, sub_error) = match collab_sub {
                Ok(out) if out.status.success() => {
                    match parse_goal_subscription_response(&out.stdout) {
                        Ok((response, id)) => {
                            let subscription = response.get("subscription").unwrap_or(&response);
                            let status = subscription
                                .get("status")
                                .and_then(Value::as_str)
                                .map(str::to_owned);
                            match status.as_deref() {
                                Some("armed") | None => (true, Some(response), Some(id), None),
                                Some(status) => (
                                    false,
                                    Some(response),
                                    Some(id),
                                    Some(format!(
                                        "GOAL_ONE_SHOT_SUBSCRIPTION_NOT_ARMED:{}",
                                        status
                                    )),
                                ),
                            }
                        }
                        Err(error) => (false, None, None, Some(error)),
                    }
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    (
                        false,
                        None,
                        None,
                        Some(format!(
                            "COLLAB_SUBSCRIBE_FAILED:exit={}{}",
                            out.status.code().unwrap_or(1),
                            if stderr.is_empty() && stdout.is_empty() {
                                String::new()
                            } else {
                                format!(":{}", if stderr.is_empty() { stdout } else { stderr })
                            }
                        )),
                    )
                }
                Err(error) => (
                    false,
                    None,
                    None,
                    Some(
                        if matches!(
                            error.as_str(),
                            "GOAL_COLLAB_COMMAND_TIMEOUT" | "GOAL_COLLAB_OUTPUT_DRAIN_TIMEOUT"
                        ) {
                            error
                        } else {
                            format!("COLLAB_UNAVAILABLE:{}", error)
                        },
                    ),
                ),
            };

            record["collab_subscribed"] = Value::Bool(collab_subscribed);
            record["collab_subscription"] = sub_details.unwrap_or(Value::Null);
            record["subscription_id"] = subscription_id.map(Value::String).unwrap_or(Value::Null);
            record["remote_state"] = Value::String(if collab_subscribed {
                "armed".into()
            } else {
                "unknown".into()
            });
            record["observed"] = Value::String(if collab_subscribed {
                "subscribed".into()
            } else {
                "unknown".into()
            });
            record["active"] = Value::Bool(collab_subscribed);
            record["error"] = sub_error.map(Value::String).unwrap_or(Value::Null);
            let subscription_not_armed_error = record["error"].as_str().and_then(|error| {
                error
                    .starts_with("GOAL_ONE_SHOT_SUBSCRIPTION_NOT_ARMED:")
                    .then_some(error.to_string())
            });
            if !collab_subscribed
                && record["subscription_id"].as_str().is_some()
                && subscription_not_armed_error.is_some()
            {
                goal_mark_recovery_required(&mut record, subscription_not_armed_error.unwrap());
            }
            record["revision"] = Value::Number(2.into());
            if let Err(error) = goal_record_write(root, &record) {
                record["active"] = Value::Bool(false);
                record["observed"] = Value::String("unknown".into());
                record["error"] = Value::String(error.clone());
                record["recovery"] = Value::String(
                    "retain the returned subscription_id and cancel it after restoring local storage".into(),
                );
                drop(_goal_lock);
                goal_fail(format_json, &error, Some(&record));
            }

            if !collab_subscribed {
                let error = record["error"].as_str().unwrap_or("GOAL_SUBSCRIBE_UNKNOWN");
                drop(_goal_lock);
                goal_fail(format_json, error, Some(&record));
            } else if format_json {
                let mut resp = record.clone();
                resp["master_prompt"] = Value::String(master_prompt);
                println!("{}", serde_json::to_string_pretty(&resp).unwrap());
            } else {
                println!("Long-horizon goal successfully registered:");
                println!("- Goal file: {}", goal_path.display());
                println!(
                    "- One-shot deadline: first trigger after {} (local rearm intent: every {}, up to {} deliveries; TTL {} seconds)",
                    interval_str, interval_str, repeat_count, ttl_seconds
                );
                println!("- Collab notification status: one-shot armed; rearm is explicit and verifiable");
                println!("\n{}", master_prompt);
            }
        }
        "status" => {
            let mut format_json = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--json" => format_json = true,
                    other => fail(format!("UNKNOWN_GOAL_STATUS_OPTION:{}", other)),
                }
            }
            let _goal_lock = match GoalLock::acquire(root, "status") {
                Ok(lock) => lock,
                Err(error) => goal_fail(format_json, &error, None),
            };
            let existing = match goal_record_read(root) {
                Ok(existing) => existing,
                Err(error) => {
                    drop(_goal_lock);
                    goal_fail(format_json, &error, None)
                }
            };
            let Some(mut record) = existing else {
                if format_json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "active": false,
                            "desired": "unknown",
                            "observed": "unknown",
                            "error": "GOAL_RECORD_NOT_FOUND",
                            "recovery": "subscribe a goal before requesting status"
                        })
                    );
                } else {
                    println!("Goal status unknown: no local goal record; subscribe a goal before requesting status.");
                }
                return;
            };
            if !record.is_object() || record["goal_id"].as_str().is_none() {
                drop(_goal_lock);
                goal_fail(
                    format_json,
                    "GOAL_RECORD_INVALID: goal_id is missing",
                    Some(&record),
                );
            }
            let old_record = record.clone();
            let subscription_id = goal_record_subscription_id(&record);
            let mut reconcile_error = None;
            if matches!(
                record["desired"].as_str(),
                Some("subscribed" | "recovery_required")
            ) {
                let by_id = subscription_id
                    .as_deref()
                    .map(|id| goal_subscription_status(root, id));
                let by_id =
                    by_id.unwrap_or_else(|| Err("GOAL_STATUS_SUBSCRIPTION_ID_MISSING".into()));
                match by_id {
                    Ok((remote_status, remote_record)) => {
                        record["remote_state"] = Value::String(remote_status.clone());
                        record["collab_subscription"] = remote_record;
                        if remote_status == "armed" {
                            record["desired"] = Value::String("subscribed".into());
                            record["observed"] = Value::String("subscribed".into());
                            record["active"] = Value::Bool(true);
                            record["collab_subscribed"] = Value::Bool(true);
                            record["error"] = Value::Null;
                        } else {
                            let error = format!(
                                "GOAL_ONE_SHOT_SUBSCRIPTION_NOT_ARMED:{}: deadline may be consumed or expired, or Collab may have restarted",
                                remote_status
                            );
                            record["desired"] = Value::String("recovery_required".into());
                            record["observed"] = Value::String(remote_status);
                            record["active"] = Value::Bool(false);
                            record["error"] = Value::String(error.clone());
                            record["recovery"] = Value::String(
                                "Rerun appsdk goal subscribe --goal <path.md> to rearm a fresh one-shot deadline; no automatic renewal is attempted".into(),
                            );
                            reconcile_error = Some(error);
                        }
                    }
                    Err(primary_error) => {
                        let canonical_subject = record["goal_id"]
                            .as_str()
                            .map(|goal_id| format!("goal:{}", goal_id))
                            .unwrap_or_default();
                        match goal_subscription_by_subject_candidates(
                            root,
                            Some(&record),
                            &canonical_subject,
                        ) {
                            Ok(Some((
                                resolved_id,
                                remote_status,
                                remote_record,
                                matched_subject,
                            ))) => {
                                let retained_subject =
                                    record["subject"].as_str().map(str::to_owned);
                                record["subscription_id"] = Value::String(resolved_id);
                                record["remote_state"] = Value::String(remote_status);
                                record["desired"] = Value::String("subscribed".into());
                                record["observed"] = Value::String("subscribed".into());
                                record["active"] = Value::Bool(true);
                                record["collab_subscribed"] = Value::Bool(true);
                                record["collab_subscription"] = remote_record;
                                record["error"] = Value::Null;
                                if retained_subject.as_deref() != Some(matched_subject.as_str()) {
                                    record["subject_migration"] = serde_json::json!({
                                        "from": retained_subject,
                                        "to": matched_subject,
                                        "status": "canonical_subject_migrated",
                                        "migrated_at": chrono::Utc::now().to_rfc3339()
                                    });
                                    record["subject"] = Value::String(matched_subject);
                                }
                                reconcile_error = Some(format!(
                                    "{}; reconciled by retained-compatible subject",
                                    primary_error
                                ));
                            }
                            Ok(None) => {
                                let error = format!(
                                    "{}; GOAL_STATUS_SUBSCRIPTION_LOST: no armed deadline subscription matches the retained or canonical subject",
                                    primary_error
                                );
                                reconcile_error = Some(error.clone());
                                record["desired"] = Value::String("recovery_required".into());
                                record["active"] = Value::Bool(false);
                                record["observed"] = Value::String("unknown".into());
                                record["remote_state"] = Value::String("unknown".into());
                                record["error"] = Value::String(error);
                            }
                            Err(subject_error) => {
                                let error = format!(
                                    "{}; GOAL_STATUS_SUBJECT_RECONCILIATION_FAILED:{}",
                                    primary_error, subject_error
                                );
                                reconcile_error = Some(error.clone());
                                record["desired"] = Value::String("recovery_required".into());
                                record["active"] = Value::Bool(false);
                                record["observed"] = Value::String("unknown".into());
                                record["remote_state"] = Value::String("unknown".into());
                                record["error"] = Value::String(error);
                            }
                        }
                        record["recovery"] = Value::String(
                            "restore Collab, then rerun appsdk goal status; if the one-shot deadline expired, rerun appsdk goal subscribe --goal <path.md> to rearm it".into(),
                        );
                    }
                }
            }
            if record != old_record {
                let revision = old_record["revision"].as_u64().unwrap_or(0);
                record["revision"] = Value::Number((revision + 1).into());
                if let Err(error) = goal_record_write(root, &record) {
                    record["active"] = Value::Bool(false);
                    record["observed"] = Value::String("unknown".into());
                    record["error"] = Value::String(error.clone());
                    drop(_goal_lock);
                    goal_fail(format_json, &error, Some(&record));
                }
            }
            if format_json {
                let payload = serde_json::json!({
                    "active": record["active"].as_bool().unwrap_or(false),
                    "desired": record["desired"].as_str().unwrap_or("unknown"),
                    "observed": record["observed"].as_str().unwrap_or("unknown"),
                    "goal_id": record["goal_id"].as_str(),
                    "goal_path": record["goal_path"].as_str(),
                    "interval": record["interval"].as_str(),
                    "subscription_id": goal_record_subscription_id(&record),
                    "collab_subscribed": record["collab_subscribed"].as_bool(),
                    "error": record["error"],
                    "reconciliation_error": reconcile_error,
                    "record": record,
                });
                println!("{}", serde_json::to_string_pretty(&payload).unwrap());
            } else {
                println!("Active Long-Horizon Goal:");
                println!(
                    "- Goal: {}",
                    record["goal_path"].as_str().unwrap_or("unknown")
                );
                println!(
                    "- Interval: {}",
                    record["interval"].as_str().unwrap_or("unknown")
                );
                println!(
                    "- Registered at: {}",
                    record["registered_at"].as_str().unwrap_or("unknown")
                );
                println!("- Active: {}", record["active"].as_bool().unwrap_or(false));
                println!(
                    "- Desired: {} | Observed: {}",
                    record["desired"].as_str().unwrap_or("unknown"),
                    record["observed"].as_str().unwrap_or("unknown")
                );
                if let Some(error) = record["error"].as_str() {
                    println!("- Error: {}", error);
                }
            }
        }
        "cancel" => {
            let mut format_json = false;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--json" => format_json = true,
                    other => fail(format!("UNKNOWN_GOAL_CANCEL_OPTION:{}", other)),
                }
            }
            let owner = match verified_goal_master(root) {
                Ok(owner) => owner,
                Err(error) => goal_fail(format_json, &error, None),
            };
            let _goal_lock = match GoalLock::acquire(root, &owner) {
                Ok(lock) => lock,
                Err(error) => goal_fail(format_json, &error, None),
            };
            let Some(mut record) = (match goal_record_read(root) {
                Ok(record) => record,
                Err(error) => {
                    drop(_goal_lock);
                    goal_fail(format_json, &error, None)
                }
            }) else {
                drop(_goal_lock);
                goal_fail(format_json, "GOAL_CANCEL_SUBSCRIPTION_MISSING", None);
            };
            if record["owner"].as_str() != Some(owner.as_str()) {
                drop(_goal_lock);
                goal_fail(
                    format_json,
                    "GOAL_CANCEL_OWNER_MISMATCH: only the recorded goal owner may cancel this subscription",
                    Some(&record),
                );
            }
            if record["desired"].as_str() == Some("unsubscribed")
                && record["observed"].as_str() == Some("cancelled")
            {
                if format_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": true,
                            "status": "cancelled",
                            "idempotent": true,
                            "subscription_id": goal_record_subscription_id(&record),
                            "revision": record["revision"],
                            "cancel_receipt": record["cancel_receipt"],
                            "record": record,
                        }))
                        .unwrap()
                    );
                } else {
                    println!("Goal subscription already cancelled.");
                }
                return;
            }
            let subject = record["subject"]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned);
            let canonical_subject = record["goal_id"]
                .as_str()
                .map(|goal_id| format!("goal:{}", goal_id))
                .unwrap_or_default();
            let mut subscription_id = goal_record_subscription_id(&record);
            if subscription_id.is_none() {
                let subject_result = goal_subscription_by_subject_candidates(
                    root,
                    Some(&record),
                    &canonical_subject,
                );
                match subject_result {
                    Ok(Some((resolved_id, remote_status, remote_record, matched_subject))) => {
                        if subject.as_deref() != Some(matched_subject.as_str()) {
                            record["subject_migration"] = serde_json::json!({
                                "from": subject.clone(),
                                "to": matched_subject,
                                "status": "canonical_subject_migrated",
                                "migrated_at": chrono::Utc::now().to_rfc3339()
                            });
                            record["subject"] = Value::String(matched_subject.clone());
                        }
                        record["subscription_id"] = Value::String(resolved_id.clone());
                        record["collab_subscription"] = remote_record;
                        record["remote_state"] = Value::String(remote_status);
                        record["observed"] = Value::String("subscribed".into());
                        record["active"] = Value::Bool(true);
                        record["revision"] =
                            Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
                        if let Err(write_error) = goal_record_write(root, &record) {
                            record["error"] = Value::String(format!(
                                "GOAL_CANCEL_SUBJECT_BIND_RECORD_WRITE_FAILED:{}",
                                write_error
                            ));
                            drop(_goal_lock);
                            goal_fail(
                                format_json,
                                record["error"].as_str().unwrap(),
                                Some(&record),
                            );
                        }
                        subscription_id = Some(resolved_id);
                    }
                    Ok(None) => {
                        let error = "GOAL_CANCEL_SUBJECT_NOT_FOUND: no armed deadline subscription matches the retained or canonical subject";
                        record["desired"] = Value::String("cancel_pending".into());
                        record["observed"] = Value::String("unknown".into());
                        record["active"] = Value::Bool(false);
                        record["error"] = Value::String(error.into());
                        record["remote_state"] = Value::String("unknown".into());
                        record["revision"] =
                            Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
                        if let Err(write_error) = goal_record_write(root, &record) {
                            record["error"] = Value::String(format!(
                                "GOAL_CANCEL_PENDING_RECORD_WRITE_FAILED:{}; original={}",
                                write_error, error
                            ));
                            drop(_goal_lock);
                            goal_fail(
                                format_json,
                                record["error"].as_str().unwrap(),
                                Some(&record),
                            );
                        }
                        drop(_goal_lock);
                        goal_fail(format_json, error, Some(&record));
                    }
                    Err(error) => {
                        record["desired"] = Value::String("cancel_pending".into());
                        record["observed"] = Value::String("unknown".into());
                        record["active"] = Value::Bool(false);
                        record["error"] = Value::String(error.clone());
                        record["remote_state"] = Value::String("unknown".into());
                        record["revision"] =
                            Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
                        if let Err(write_error) = goal_record_write(root, &record) {
                            record["error"] = Value::String(format!(
                                "GOAL_CANCEL_PENDING_RECORD_WRITE_FAILED:{}; original={}",
                                write_error, error
                            ));
                            drop(_goal_lock);
                            goal_fail(
                                format_json,
                                record["error"].as_str().unwrap(),
                                Some(&record),
                            );
                        }
                        drop(_goal_lock);
                        goal_fail(format_json, &error, Some(&record));
                    }
                }
            }
            let subscription_id = subscription_id.expect("resolved goal subscription ID");
            let mut cancel_result = goal_cancel_subscription(root, &subscription_id);
            if let Err(original_error) = cancel_result {
                if subject.is_some() || !canonical_subject.is_empty() {
                    cancel_result = match goal_subscription_by_subject_candidates(
                        root,
                        Some(&record),
                        &canonical_subject,
                    ) {
                        Ok(Some((resolved_id, remote_status, remote_record, matched_subject))) => {
                            if resolved_id != subscription_id {
                                record["subscription_id"] = Value::String(resolved_id.clone());
                                record["collab_subscription"] = remote_record;
                                record["remote_state"] = Value::String(remote_status);
                                if subject.as_deref() != Some(matched_subject.as_str()) {
                                    record["subject_migration"] = serde_json::json!({
                                        "from": subject.clone(),
                                        "to": matched_subject,
                                        "status": "canonical_subject_migrated",
                                        "migrated_at": chrono::Utc::now().to_rfc3339()
                                    });
                                    record["subject"] = Value::String(matched_subject);
                                }
                                record["revision"] = Value::Number(
                                    (record["revision"].as_u64().unwrap_or(0) + 1).into(),
                                );
                                if let Err(write_error) = goal_record_write(root, &record) {
                                    let error = format!(
                                        "GOAL_CANCEL_RECONCILIATION_RECORD_WRITE_FAILED:{}; original={}",
                                        write_error, original_error
                                    );
                                    Err(error)
                                } else {
                                    goal_cancel_subscription(root, &resolved_id).map_err(|retry| {
                                        format!("{}; retry={}", original_error, retry)
                                    })
                                }
                            } else {
                                goal_cancel_subscription(root, &resolved_id)
                                    .map_err(|retry| format!("{}; retry={}", original_error, retry))
                            }
                        }
                        Ok(None) => Err(format!(
                            "{}; GOAL_CANCEL_SUBJECT_NOT_FOUND: remote state remains unknown",
                            original_error
                        )),
                        Err(reconcile_error) => Err(format!(
                            "{}; GOAL_CANCEL_RECONCILIATION_FAILED:{}",
                            original_error, reconcile_error
                        )),
                    };
                } else {
                    cancel_result = Err(format!("{}; GOAL_CANCEL_SUBJECT_MISSING", original_error));
                }
            }
            let cancel_receipt = match cancel_result {
                Ok(receipt) => receipt,
                Err(error) => {
                    record["desired"] = Value::String("cancel_pending".into());
                    record["observed"] = Value::String("unknown".into());
                    record["active"] = Value::Bool(false);
                    record["error"] = Value::String(error.clone());
                    record["remote_state"] = Value::String("unknown".into());
                    record["revision"] =
                        Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
                    if let Err(write_error) = goal_record_write(root, &record) {
                        record["error"] = Value::String(format!(
                            "GOAL_CANCEL_PENDING_RECORD_WRITE_FAILED:{}; original={}",
                            write_error, error
                        ));
                        drop(_goal_lock);
                        goal_fail(
                            format_json,
                            record["error"].as_str().unwrap(),
                            Some(&record),
                        );
                    }
                    drop(_goal_lock);
                    goal_fail(format_json, &error, Some(&record));
                }
            };
            record["desired"] = Value::String("unsubscribed".into());
            record["observed"] = Value::String("cancelled".into());
            record["active"] = Value::Bool(false);
            record["remote_state"] = Value::String("cancelled".into());
            record["error"] = Value::Null;
            record["cancel_receipt"] = cancel_receipt;
            record["cancelled_at"] = Value::String(chrono::Utc::now().to_rfc3339());
            record["revision"] =
                Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
            if let Err(error) = goal_record_write(root, &record) {
                record["desired"] = Value::String("cancel_pending".into());
                record["observed"] = Value::String("unknown".into());
                record["active"] = Value::Bool(false);
                record["error"] =
                    Value::String(format!("GOAL_CANCEL_RECORD_WRITE_FAILED:{}", error));
                record["revision"] =
                    Value::Number((record["revision"].as_u64().unwrap_or(0) + 1).into());
                drop(_goal_lock);
                goal_fail(
                    format_json,
                    record["error"].as_str().unwrap(),
                    Some(&record),
                );
            }
            if format_json {
                println!(
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "status": "cancelled",
                        "subscription_id": goal_record_subscription_id(&record),
                        "revision": record["revision"],
                        "cancel_receipt": record["cancel_receipt"],
                        "record": record,
                    })
                );
            } else {
                let final_id =
                    goal_record_subscription_id(&record).unwrap_or_else(|| subscription_id.clone());
                println!("Goal subscription cancelled: {}", final_id);
            }
        }
        "prompt" => {
            let mut goal_file: Option<String> = None;
            let mut interval_str = "10m".to_string();

            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "-g" | "--goal" => {
                        goal_file = Some(args.next().unwrap_or_else(|| fail("MISSING_GOAL_ARG")));
                    }
                    "-i" | "--interval" | "--every" => {
                        interval_str = args.next().unwrap_or_else(|| fail("MISSING_INTERVAL_ARG"));
                    }
                    _ => {}
                }
            }

            let raw_goal = goal_file.unwrap_or_else(|| {
                fail("USAGE: appsdk goal prompt --goal <path.md> [--interval <duration>]")
            });
            if !raw_goal.to_lowercase().ends_with(".md") {
                fail(format!(
                    "GOAL_PATH_MUST_BE_MD_FILE: '{}' is not a markdown file (.md)",
                    raw_goal
                ));
            }
            let goal_path = if Path::new(&raw_goal).is_absolute() {
                PathBuf::from(&raw_goal)
            } else {
                root.join(&raw_goal)
            };
            if !goal_path.exists() || !goal_path.is_file() {
                fail(format!(
                    "GOAL_FILE_NOT_FOUND: '{}' does not exist or is not a file",
                    goal_path.display()
                ));
            }
            if let Err(error) = verified_goal_master(root) {
                goal_fail(false, &error, None);
            }

            let prompt = generate_long_horizon_master_prompt(&goal_path, &interval_str);
            println!("{}", prompt);
        }
        _ => fail(format!("UNKNOWN_GOAL_SUBCOMMAND:{}", sub)),
    }
}

fn handle_task_command<I>(root: &Path, mut args: I)
where
    I: Iterator<Item = String>,
{
    let sub = match args.next() {
        Some(s) => s,
        None => {
            let status = Command::new("collab")
                .arg("task")
                .current_dir(root)
                .status()
                .unwrap_or_else(|e| fail(format!("COLLAB_UNAVAILABLE:{}", e)));
            std::process::exit(status.code().unwrap_or(1));
        }
    };

    if sub == "block" {
        let task_id = args
            .next()
            .unwrap_or_else(|| fail("USAGE: appsdk task block <id> [--reason <text>] [--json]"));
        let mut reason: Option<String> = None;
        let mut format_json = false;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--reason" | "-m" | "--next" => {
                    reason = Some(args.next().unwrap_or_else(|| fail("MISSING_REASON_ARG")));
                }
                "--json" => format_json = true,
                _ => {}
            }
        }

        // 1. Invoke collab task block to update durable state
        let mut block_cmd = Command::new("collab");
        block_cmd.args(["task", "block", &task_id]);
        if let Some(r) = &reason {
            block_cmd.args(["--next", r]);
        }
        block_cmd.current_dir(root);
        let block_out = match block_cmd.output() {
            Ok(out) => out,
            Err(e) => fail(format!("COLLAB_UNAVAILABLE:{}", e)),
        };
        if !block_out.status.success() {
            let code = block_out.status.code().unwrap_or(1);
            let stderr = String::from_utf8_lossy(&block_out.stderr)
                .trim()
                .to_string();
            let stdout = String::from_utf8_lossy(&block_out.stdout)
                .trim()
                .to_string();
            eprintln!(
                "COLLAB_TASK_BLOCK_FAILED:exit={}{}{}",
                code,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(":{}", stderr)
                },
                if stdout.is_empty() {
                    String::new()
                } else {
                    format!(":{}", stdout)
                }
            );
            std::process::exit(code);
        }
        let block_result = String::from_utf8_lossy(&block_out.stdout)
            .trim()
            .to_string();

        // 2. Construct the mandatory governance reminder. Blocking a task does
        // not implicitly close all notifications; only the underlying Collab
        // command may change precise subscription policy.
        let notice_title = format!("【AppSDK 任务阻塞门禁提醒】任务 '{}' 已调用 collab task block，通知策略以 Collab 响应为准。", task_id);
        let notice_body = r#"================================================================================
【重要门禁与合规约束】
1. AppSDK 的问题可以报 bug：若阻断由 AppSDK 框架缺陷导致（CLI 异常、verify 误报、准入阻断），
   必须立即上报 upstream 缺陷系统：
   appsdk bug new --upstream -t "[SDK Bug] <简述>" -m "<复现与上下文>" -l "P0,cli"
2. 合法等待必须写清原因、责任人、解除条件和恢复触发：
   外部依赖、资源占用、凭证/批准缺失、跨 owner 决策等可以进入 waiting/blocked，
   但严禁只写“blocked”而不带恢复方案，也严禁因任务难就空等。
3. 请立即核实等待性质：无法自己解除时，把具体方案交给 master；master 必须在周期内接管、改派或强制关闭。
================================================================================"#;

        if format_json {
            let resp = serde_json::json!({
                "ok": true,
                "task_id": task_id,
                "status": "blocked",
                "reminders_stopped": false,
                "reason": reason,
                "collab_result": block_result,
                "rule": "blocked/waiting must include cause, owner, unblock condition, and recovery trigger; master owns resolution",
                "notice": format!("{}\n{}", notice_title, notice_body)
            });
            println!("{}", serde_json::to_string_pretty(&resp).unwrap());
        } else {
            println!(
                "{}\n{}\n{}{}",
                notice_title,
                notice_body,
                if block_result.is_empty() {
                    String::new()
                } else {
                    "\nCollab result:\n".to_string()
                },
                block_result
            );
        }
    } else {
        let mut rest_args = vec!["task".to_string(), sub];
        rest_args.extend(args);
        let status = Command::new("collab")
            .args(&rest_args)
            .current_dir(root)
            .status()
            .unwrap_or_else(|e| fail(format!("COLLAB_UNAVAILABLE:{}", e)));
        std::process::exit(status.code().unwrap_or(1));
    }
}

const CLI_USAGE: &str = "Usage: appsdk <command> [project] [options]\n\nProject-scoped commands default to the current working directory. An explicit project path remains optional.";

fn is_help(value: &str) -> bool {
    matches!(value, "help" | "--help" | "-h")
}

fn print_cli_help(command: Option<&str>) {
    let usage = match command {
        Some("verify") => {
            "Usage: appsdk verify [project]\n       appsdk verify --admission [project]\n       appsdk verify --review-admission [project] --module <id>"
        }
        Some("compile") => "Usage: appsdk compile [project] [--module <id>]",
        Some("compile-module") => "Usage: appsdk compile-module [project] --module <id>",
        Some("produce-lifecycle-records") => {
            "Usage: appsdk produce-lifecycle-records [project] --module <id> --input <json>"
        }
        Some("produce-lifecycle-chain") => {
            "Usage: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>"
        }
        Some("pin-lock") => "Usage: appsdk pin-lock [project] --binary <path>",
        Some("reset-governance") => {
            "Usage: appsdk reset-governance [project] --discard-legacy"
        }
        Some("init") => {
            "Usage: appsdk init [workspace] [--project-root <relative-path>] [--fresh --discard-legacy]"
        }
        Some("prepare") => "Usage: appsdk prepare [workspace]",
        Some("new") => "Usage: appsdk new [project]",
        Some("memory") | Some("project-memory") => {
            "Usage: appsdk memory <entry|query|get|review|promote|migrate|import|reentry|index|export|compact|verify> [project]"
        }
        Some("bug") => {
            "Usage: appsdk bug <new|list|show|comment|close|webui> [options]"
        }
        Some("setup-deps") => {
            "Usage: appsdk setup-deps [--check]"
        }
        Some("goal") => {
            "Usage: appsdk goal <subscribe|status|cancel|prompt> [options]\n       appsdk goal subscribe --goal <path.md> [--interval <duration>]\n       appsdk goal prompt --goal <path.md>"
        }
        Some("task") => {
            "Usage: appsdk task <block|register|relocate|update|wait|deliver|close|status> [options]\n       appsdk task block <id> [--reason <text>]"
        }
        _ => CLI_USAGE,
    };
    println!("{usage}\n\nNo project-root environment variable is required.");
}

fn project_root_or_cwd<I>(args: &mut std::iter::Peekable<I>) -> PathBuf
where
    I: Iterator<Item = String>,
{
    if args.peek().is_some_and(|value| value.starts_with('-')) {
        PathBuf::from(".")
    } else {
        PathBuf::from(args.next().unwrap_or_else(|| ".".into()))
    }
}

fn main() {
    let argv = env::args().skip(1).collect::<Vec<_>>();
    // Collab owns configuration interpretation, coordination, and subagent runtime truth.
    // Forward argv/environment unchanged; do not create an AppSDK registry.
    if argv
        .first()
        .is_some_and(|arg| arg == "subagent" || arg == "config" || arg == "collab")
    {
        let collab_args: &[String] = if argv[0] == "collab" {
            &argv[1..]
        } else {
            &argv[..]
        };
        let status = Command::new("collab")
            .args(collab_args)
            .status()
            .unwrap_or_else(|error| fail(format!("COLLAB_UNAVAILABLE:{error}")));
        std::process::exit(status.code().unwrap_or(1));
    }
    if argv.is_empty() || is_help(&argv[0]) {
        print_cli_help(None);
        return;
    }
    if argv.len() == 2 && is_help(&argv[1]) && argv[0] != "guide" {
        print_cli_help(Some(&argv[0]));
        return;
    }
    let mut args = argv.into_iter().peekable();
    match args.next().as_deref() {
        Some("version") => println!("appsdk 0.1.6 (rust)"),
        Some("verify-sdk-source-registry") => {
            assert_sdk_source_registry(Path::new(&args.next().unwrap_or_else(|| ".".into())))
        }
        Some("verify") => {
            if args.peek().is_some_and(|value| value == "--admission") {
                args.next();
                let root = project_root_or_cwd(&mut args);
                if args.next().is_some() {
                    fail("USAGE: appsdk verify --admission [project]");
                }
                verify(&root, true);
            } else if args
                .peek()
                .is_some_and(|value| value == "--review-admission")
            {
                args.next();
                let root = project_root_or_cwd(&mut args);
                if args.next().as_deref() != Some("--module") {
                    fail("USAGE: appsdk verify --review-admission [project] --module <id>");
                }
                let module_id = args.next().unwrap_or_else(|| {
                    fail("USAGE: appsdk verify --review-admission [project] --module <id>")
                });
                if args.next().is_some() {
                    fail("USAGE: appsdk verify --review-admission [project] --module <id>");
                }
                verify_review_admission(&root, &module_id);
            } else {
                let root = project_root_or_cwd(&mut args);
                if args.next().is_some() {
                    fail("USAGE: appsdk verify [project]");
                }
                verify(&root, false);
            }
        }
        Some("guide") => guidance::run(&mut args),
        Some("memory") | Some("project-memory") => memory::run(&mut args),
        Some("bug") => {
            let mut root = PathBuf::from(".");
            if let Some(first) = args.peek() {
                if !matches!(
                    first.as_str(),
                    "new"
                        | "list"
                        | "show"
                        | "comment"
                        | "close"
                        | "webui"
                        | "help"
                        | "--help"
                        | "-h"
                ) && !first.starts_with('-')
                {
                    root = PathBuf::from(args.next().unwrap());
                }
            }
            handle_bug_command(&root, args);
        }
        Some("setup-deps") => {
            let check_only = args.peek().is_some_and(|a| a == "--check");
            setup_deps(check_only);
        }
        Some("goal") => {
            let mut root = PathBuf::from(".");
            if let Some(first) = args.peek() {
                if !matches!(
                    first.as_str(),
                    "subscribe"
                        | "register"
                        | "status"
                        | "cancel"
                        | "prompt"
                        | "help"
                        | "--help"
                        | "-h"
                ) && !first.starts_with('-')
                {
                    root = PathBuf::from(args.next().unwrap());
                }
            }
            handle_goal_command(&root, args);
        }
        Some("longhorizon") | Some("long-horizon") => {
            let mut root = PathBuf::from(".");
            if let Some(first) = args.peek() {
                // Only an existing directory is a project path; anything else is
                // a subcommand and must reach the handler so typos surface.
                if !first.starts_with('-') && Path::new(first).is_dir() {
                    root = PathBuf::from(args.next().unwrap());
                }
            }
            handle_longhorizon_command(&root, args);
        }
        Some("task") => {
            let mut root = PathBuf::from(".");
            if let Some(first) = args.peek() {
                if !matches!(
                    first.as_str(),
                    "block"
                        | "register"
                        | "relocate"
                        | "update"
                        | "wait"
                        | "deliver"
                        | "close"
                        | "status"
                        | "help"
                        | "--help"
                        | "-h"
                ) && !first.starts_with('-')
                {
                    root = PathBuf::from(args.next().unwrap());
                }
            }
            handle_task_command(&root, args);
        }
        Some("pin-lock") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--binary") {
                fail("USAGE: appsdk pin-lock [project] --binary <path>");
            }
            let binary = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk pin-lock [project] --binary <path>"));
            if args.next().is_some() {
                fail("USAGE: appsdk pin-lock [project] --binary <path>");
            }
            pin_lock(&root, Path::new(&binary));
        }
        Some("reset-governance") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--discard-legacy") || args.next().is_some() {
                fail("USAGE: appsdk reset-governance [project] --discard-legacy");
            }
            reset_governance(&root, true);
        }
        Some("compile") => {
            let root = project_root_or_cwd(&mut args);
            if args.peek().is_some_and(|value| value == "--module") {
                args.next();
                let module = args
                    .next()
                    .unwrap_or_else(|| fail("USAGE: appsdk compile [project] [--module <id>]"));
                if args.next().is_some() {
                    fail("USAGE: appsdk compile [project] [--module <id>]");
                }
                compile_module(&root, &module);
            } else {
                if args.next().is_some() {
                    fail("USAGE: appsdk compile [project] [--module <id>]");
                }
                compile(&root);
            }
        }
        Some("compile-module") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk compile-module [project] --module <id>");
            }
            let module = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk compile-module [project] --module <id>"));
            if args.next().is_some() {
                fail("USAGE: appsdk compile-module [project] --module <id>");
            }
            compile_module(&root, &module);
        }
        Some("begin-version") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>");
            }
            let module_id = args.next().unwrap_or_else(|| fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>"));
            if args.next().as_deref() != Some("--from") {
                fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>");
            }
            let from = args.next().unwrap_or_else(|| fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>"));
            if args.next().as_deref() != Some("--to") {
                fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>");
            }
            let to = args.next().unwrap_or_else(|| fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>"));
            if args.next().is_some() {
                fail("USAGE: appsdk begin-version [project] --module <id> --from <version> --to <version>");
            }
            begin_version(&root, &module_id, &from, &to);
        }
        Some("rehydrate-frozen") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk rehydrate-frozen [project] --module <id>");
            }
            let module_id = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk rehydrate-frozen [project] --module <id>"));
            if args.next().is_some() {
                fail("USAGE: appsdk rehydrate-frozen [project] --module <id>");
            }
            rehydrate_frozen(&root, &module_id);
        }
        Some("promote") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--to") {
                fail("USAGE: appsdk promote [project] --to <stage>");
            }
            let stage = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk promote [project] --to <stage>"));
            if args.next().is_some() {
                fail("USAGE: appsdk promote [project] --to <stage>");
            }
            promote(&root, &stage);
        }
        Some("promote-module") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk promote-module [project] --module <id> --to <stage>");
            }
            let module_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk promote-module [project] --module <id> --to <stage>")
            });
            if args.next().as_deref() != Some("--to") {
                fail("USAGE: appsdk promote-module [project] --module <id> --to <stage>");
            }
            let target = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk promote-module [project] --module <id> --to <stage>")
            });
            if args.next().is_some() {
                fail("USAGE: appsdk promote-module [project] --module <id> --to <stage>");
            }
            promote_module(&root, &module_id, &target);
        }
        Some("produce-lifecycle-records") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk produce-lifecycle-records [project] --module <id> --input <json>");
            }
            let module_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk produce-lifecycle-records [project] --module <id> --input <json>")
            });
            if args.next().as_deref() != Some("--input") {
                fail("USAGE: appsdk produce-lifecycle-records [project] --module <id> --input <json>");
            }
            let input = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk produce-lifecycle-records [project] --module <id> --input <json>")
            });
            if args.next().is_some() {
                fail("USAGE: appsdk produce-lifecycle-records [project] --module <id> --input <json>");
            }
            produce_lifecycle_records(&root, &module_id, &input);
        }
        Some("produce-lifecycle-chain") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>");
            }
            let module_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>")
            });
            if args.next().as_deref() != Some("--phase") {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>");
            }
            let phase = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>")
            });
            if args.next().as_deref() != Some("--input") {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>");
            }
            let input = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>")
            });
            if args.next().is_some() {
                fail("USAGE: appsdk produce-lifecycle-chain [project] --module <id> --phase <architecture|effectiveness|merge|promotion> --input <json>");
            }
            produce_lifecycle_chain(&root, &module_id, &phase, &input);
        }
        Some("freeze") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk freeze [project] --module <id>");
            }
            let module = args
                .next()
                .unwrap_or_else(|| fail("USAGE: appsdk freeze [project] --module <id>"));
            if args.next().is_some() {
                fail("USAGE: appsdk freeze [project] --module <id>");
            }
            freeze_module(&root, &module);
        }
        Some("publish-active") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().as_deref() != Some("--module") {
                fail("USAGE: appsdk publish-active [project] --module <id> --version <version>");
            }
            let module_id = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk publish-active [project] --module <id> --version <version>")
            });
            if args.next().as_deref() != Some("--version") {
                fail("USAGE: appsdk publish-active [project] --module <id> --version <version>");
            }
            let version = args.next().unwrap_or_else(|| {
                fail("USAGE: appsdk publish-active [project] --module <id> --version <version>")
            });
            if args.next().is_some() {
                fail("USAGE: appsdk publish-active [project] --module <id> --version <version>");
            }
            publish_active(&root, &module_id, &version);
        }
        Some("new") => {
            let root = project_root_or_cwd(&mut args);
            if args.next().is_some() {
                fail("USAGE: appsdk new [project]");
            }
            new_project(&root);
        }
        Some("init") => {
            let workspace = project_root_or_cwd(&mut args);
            let usage =
                "USAGE: appsdk init [workspace] [--project-root <relative-path>] [--fresh --discard-legacy]";
            let mut project_root = None;
            let mut fresh = false;
            let mut discard_legacy = false;
            while let Some(option) = args.next() {
                match option.as_str() {
                    "--project-root" => {
                        if project_root.is_some() {
                            fail(usage);
                        }
                        let value = args.next().unwrap_or_else(|| fail(usage));
                        if value.starts_with('-') {
                            fail(usage);
                        }
                        project_root = Some(value);
                    }
                    "--fresh" => {
                        if fresh {
                            fail(usage);
                        }
                        fresh = true;
                    }
                    "--discard-legacy" => {
                        if discard_legacy {
                            fail(usage);
                        }
                        discard_legacy = true;
                    }
                    _ => fail(usage),
                }
            }
            if fresh && !discard_legacy {
                fail("INIT_FRESH_REQUIRES_DISCARD_LEGACY_CONFIRMATION");
            }
            if discard_legacy && !fresh {
                fail("INIT_DISCARD_LEGACY_REQUIRES_FRESH");
            }
            let workspace_path = workspace.as_path();
            if fresh {
                let root = canonical_init_target(workspace_path, project_root.as_deref());
                init_project(&root, true, discard_legacy);
            } else if let Some(root) = existing_init_target(workspace_path, project_root.as_deref())
            {
                init_project(&root, false, false);
            } else {
                let (preparation, preparation_workspace) = read_init_preparation(workspace_path);
                let prepared_root = preparation
                    .get("project_root")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| fail("PREPARATION_PROJECT_ROOT_MISSING"));
                if let Some(explicit_root) = project_root.as_deref() {
                    let _ = resolve_init_target(&preparation_workspace, Some(explicit_root));
                }
                if project_root
                    .as_deref()
                    .is_some_and(|root| root != prepared_root)
                {
                    fail("PREPARATION_PROJECT_ROOT_MISMATCH");
                }
                let root = resolve_init_target(&preparation_workspace, Some(prepared_root));
                init_project(&root, false, false);
            }
        }
        Some("prepare") => {
            let workspace = project_root_or_cwd(&mut args);
            if args.next().is_some() {
                fail("USAGE: appsdk prepare [workspace]");
            }
            prepare_project(&workspace);
        }
        Some("communication") | Some("comm") => {
            if let Err(error) = communication::run_cli(args.collect()) {
                fail(error.to_string());
            }
        }
        _ => fail(CLI_USAGE),
    }
}

#[allow(dead_code)]
fn digest_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}
