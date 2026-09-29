use super::*;

pub(super) fn canonical_governance_map(name: &str) -> &'static str {
    match name {
        "resource-map.json" => include_str!("../../../contracts/maps/resource-map.json"),
        "function-map.json" => include_str!("../../../contracts/maps/function-map.json"),
        "mainline-call-map.json" => include_str!("../../../contracts/maps/mainline-call-map.json"),
        "verification-map.json" => include_str!("../../../contracts/maps/verification-map.json"),
        _ => fail("UNKNOWN_GOVERNANCE_MAP"),
    }
}

pub(super) fn historical_governance_map(version: &str, name: &str) -> &'static str {
    match (version, name) {
        ("0.1.5", "resource-map.json") => {
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/resource-map.json")
        }
        ("0.1.5", "function-map.json") => {
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/function-map.json")
        }
        ("0.1.5", "mainline-call-map.json") => {
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/mainline-call-map.json")
        }
        ("0.1.5", "verification-map.json") => {
            include_str!("../../../contracts/migrations/0.1.5/governance-maps/verification-map.json")
        }
        ("0.1.6", "resource-map.json") => {
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/resource-map.json")
        }
        ("0.1.6", "function-map.json") => {
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/function-map.json")
        }
        ("0.1.6", "mainline-call-map.json") => {
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/mainline-call-map.json")
        }
        ("0.1.6", "verification-map.json") => {
            include_str!("../../../contracts/migrations/0.1.6/governance-maps/verification-map.json")
        }
        ("0.1.0007", "resource-map.json") => include_str!("../../../contracts/migrations/0.1.0007/governance-maps/resource-map.json"),
        ("0.1.0007", "function-map.json") => include_str!("../../../contracts/migrations/0.1.0007/governance-maps/function-map.json"),
        ("0.1.0007", "mainline-call-map.json") => include_str!("../../../contracts/migrations/0.1.0007/governance-maps/mainline-call-map.json"),
        ("0.1.0007", "verification-map.json") => include_str!("../../../contracts/migrations/0.1.0007/governance-maps/verification-map.json"),
        _ => fail("UNKNOWN_GOVERNANCE_MAP"),
    }
}

pub(super) fn sdk_map_migration_manifest(step: &str) -> Value {
    let (manifest_text, source_version, target_version) = match step {
        "0.1.5-to-0.1.6" => (SDK_MAP_MIGRATION_0_1_5_TO_0_1_6, "0.1.5", "0.1.6"),
        "0.1.6-to-0.1.0007" => (SDK_MAP_MIGRATION_0_1_6_TO_0_1_0007, "0.1.6", "0.1.0007"),
        "0.1.0007-to-0.1.0008" => (SDK_MAP_MIGRATION_0_1_0007_TO_0_1_0008, "0.1.0007", "0.1.0008"),
        _ => fail("UNKNOWN_SDK_MAP_MIGRATION_STEP"),
    };
    let manifest: Value = serde_json::from_str(manifest_text)
        .unwrap_or_else(|_| fail("INVALID_SDK_MAP_MIGRATION_MANIFEST"));
    let migration_id = format!("appsdk-{step}");
    let snapshot_root = format!(".appsdk/migrations/{step}/maps");
    let record_path = format!(".appsdk/migrations/{step}/record.json");
    if manifest.get("schema_version").and_then(Value::as_u64) != Some(1)
        || manifest.get("migration_id").and_then(Value::as_str) != Some(migration_id.as_str())
        || manifest.get("source_version").and_then(Value::as_str) != Some(source_version)
        || manifest.get("target_version").and_then(Value::as_str) != Some(target_version)
        || manifest.get("materialization").and_then(Value::as_str)
            != Some("pin_lock_when_migrating")
        || manifest.get("snapshot_root").and_then(Value::as_str) != Some(snapshot_root.as_str())
        || manifest.get("record_path").and_then(Value::as_str) != Some(record_path.as_str())
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
            != Some(
                digest_bytes(historical_governance_map(source_version, name).as_bytes()).as_str(),
            )
            || entry.get("target_digest").and_then(Value::as_str)
                != Some(
                    digest_bytes(if target_version == SDK_VERSION {
                        canonical_governance_map(name).as_bytes()
                    } else {
                        historical_governance_map(target_version, name).as_bytes()
                    })
                    .as_str(),
                )
        {
            fail(format!(
                "SDK_MAP_MIGRATION_MANIFEST_DIGEST_MISMATCH:{}",
                name
            ));
        }
    }
    manifest
}

pub(super) fn sdk_bundle_manifest_resources() -> Value {
    serde_json::from_str::<Value>(SDK_BUNDLE_MANIFEST)
        .unwrap_or_else(|_| fail("INVALID_SDK_BUNDLE_MANIFEST"))
        .get("resources")
        .cloned()
        .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_MANIFEST"))
}

pub(super) fn sdk_resource_install_relative(source: &str, class: &str) -> String {
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

pub(super) fn sdk_bundle_resource_entries() -> Vec<(String, String, &'static str)> {
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

pub(super) fn sdk_bundle_digest() -> String {
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

pub(super) fn assert_bundle_manifest() {
    let manifest: Value = serde_json::from_str(SDK_BUNDLE_MANIFEST)
        .unwrap_or_else(|_| fail("INVALID_SDK_BUNDLE_MANIFEST"));
    if manifest.get("schema_version").and_then(Value::as_u64) != Some(1)
        || manifest.get("sdk").and_then(Value::as_str) != Some("appsdk")
        || manifest.get("version").and_then(Value::as_str) != Some(SDK_VERSION)
        || manifest.get("runtime_entrypoint").and_then(Value::as_str) != Some("rust-binary")
    {
        fail("INVALID_SDK_BUNDLE_MANIFEST");
    }
    let _ = sdk_bundle_resource_entries();
}

pub(super) fn install_bundle_resources(root: &Path) {
    assert_bundle_manifest();
    let mut installed = Vec::new();
    for (source, class, embedded_content) in sdk_bundle_resource_entries() {
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
        let source_path = root.join(&source);
        let project_record_contract = class == "contracts"
            && source.starts_with("contracts/records/")
            && CANONICAL_RECORD_CONTRACTS.contains(&source.as_str());
        let content = if project_record_contract && source_path.is_file() {
            assert_no_symlink_components(root, &source_path, "sdk_resource_source");
            fs::read(&source_path).unwrap_or_else(|_| fail("SDK_RESOURCE_SOURCE_READ_FAILED"))
        } else {
            embedded_content.as_bytes().to_vec()
        };
        atomic_write_bytes(&target, &content, "SDK_RESOURCE_WRITE_FAILED");
        installed.push(serde_json::json!({
            "source": source,
            "class": class,
            "path": target.strip_prefix(root).unwrap().to_string_lossy(),
            "digest": digest_bytes(&content)
        }));
    }
    let record = serde_json::json!({
        "schema_version": 1,
        "sdk": "appsdk",
        "version": SDK_VERSION,
        "bundle_digest": sdk_bundle_digest(),
        "manifest_digest": digest_bytes(SDK_BUNDLE_MANIFEST.as_bytes()),
        "resources": installed
    });
    let record_path = root.join(".appsdk/sdk-resources.json");
    atomic_write_json(&record_path, &record, "SDK_RESOURCE_RECORD_WRITE_FAILED");
}

pub(super) fn reconcile_authoring_bundle_manifest(root: &Path) {
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

pub(super) fn fail(message: impl AsRef<str>) -> ! {
    eprintln!("{}", message.as_ref());
    std::process::exit(1);
}

pub(super) fn write_embedded_contract(root: &Path, relative: &str, content: &str) {
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

pub(super) fn bootstrap_contracts(root: &Path) {
    write_embedded_contract(
        root,
        ".appsdk/contracts/development-scenarios.manifest.json",
        include_str!("../../../contracts/development-scenarios.manifest.json"),
    );
    write_embedded_contract(
        root,
        ".appsdk/maps/resource-map.json",
        include_str!("../../../contracts/maps/resource-map.json"),
    );
    write_embedded_contract(
        root,
        ".appsdk/maps/function-map.json",
        include_str!("../../../contracts/maps/function-map.json"),
    );
    write_embedded_contract(
        root,
        ".appsdk/maps/mainline-call-map.json",
        include_str!("../../../contracts/maps/mainline-call-map.json"),
    );
    write_embedded_contract(
        root,
        ".appsdk/maps/verification-map.json",
        include_str!("../../../contracts/maps/verification-map.json"),
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
    for &(relative, _, content) in SDK_BUNDLE_RESOURCES
        .iter()
        .filter(|(path, class, _)| *class == "contracts" && path.starts_with("contracts/records/"))
    {
        write_embedded_contract(root, relative, content);
    }
    for (relative, content) in [
        (
            "contracts/lifecycle-state-machines.json",
            include_str!("../../../contracts/lifecycle-state-machines.json"),
        ),
        (
            "contracts/lifecycle-state-machines.manifest.json",
            include_str!("../../../contracts/lifecycle-state-machines.manifest.json"),
        ),
        (
            "contracts/goal-clarification-state-machine.json",
            include_str!("../../../contracts/goal-clarification-state-machine.json"),
        ),
    ] {
        write_embedded_contract(root, relative, content);
    }
}

pub(super) fn project_file(root: &Path) -> PathBuf {
    root.join(".appsdk").join("project.json")
}

pub(super) fn read_project(root: &Path) -> Value {
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

pub(super) fn assert_project_root_safe(root: &Path) {
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

pub(super) fn freeze_record_name(module_id: &str) -> String {
    format!("freeze-record-{}.json", module_id)
}

pub(super) fn module_record_name(kind: &str, module_id: &str) -> String {
    format!("{}-{}.json", kind, module_id)
}

pub(super) fn retire_record_snapshot(
    root: &Path,
    records_root: &Path,
    module_id: &str,
    kind: &'static str,
    file_name: &'static str,
) -> RetireRecordSnapshot {
    let source = records_root.join(module_record_name(kind, module_id));
    assert_no_symlink_components(root, &source, "retire_record");
    let metadata = fs::symlink_metadata(&source).unwrap_or_else(|error| {
        if error.kind() == ErrorKind::NotFound {
            fail("RETIRE_RECORD_SET_INCOMPLETE");
        }
        fail("RETIRE_RECORD_READ_FAILED");
    });
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        fail("RETIRE_RECORD_READ_FAILED");
    }
    let bytes = fs::read(&source).unwrap_or_else(|_| fail("RETIRE_RECORD_READ_FAILED"));
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| fail("RETIRE_RECORD_INVALID"));
    RetireRecordSnapshot {
        kind,
        file_name,
        source,
        bytes,
        value,
    }
}

pub(super) fn retire_validate_worktree(root: &Path) {
    retire_validate_reentry_worktree(root, &[]);
}

pub(super) fn retire_validate_reentry_worktree(root: &Path, allowed: &[PathBuf]) {
    let branch = git_value(
        root,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        "RETIRE_WORKTREE_BRANCH_REQUIRED",
    );
    if !branch.starts_with("codex/") {
        fail("RETIRE_WORKTREE_OWNER_REQUIRED");
    }
    let status = git_value(
        root,
        &["status", "--porcelain", "--untracked-files=all", "--", "."],
        "RETIRE_VCS_UNAVAILABLE",
    );
    if status.is_empty() {
        return;
    }
    let allowed = allowed
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or_else(|_| fail("RETIRE_WORKTREE_DIRTY"))
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<BTreeSet<_>>();
    for line in status.lines() {
        let path_text = line.get(2..).unwrap_or("").trim();
        for path in path_text.split(" -> ") {
            if !allowed.contains(path) {
                fail(format!("RETIRE_WORKTREE_DIRTY:{}", path));
            }
        }
    }
}

pub(super) fn retire_verify_candidate_identity(root: &Path, candidate_commit: &str, candidate_tree: &str) {
    let resolved_commit = git_value(
        root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{}^{{commit}}", candidate_commit),
        ],
        "RETIRE_CANDIDATE_COMMIT_INVALID",
    );
    if resolved_commit != candidate_commit {
        fail("RETIRE_CANDIDATE_COMMIT_MISMATCH");
    }
    let resolved_tree = git_value(
        root,
        &["rev-parse", &format!("{}^{{tree}}", candidate_commit)],
        "RETIRE_CANDIDATE_TREE_INVALID",
    );
    if resolved_tree != candidate_tree {
        fail("RETIRE_CANDIDATE_TREE_MISMATCH");
    }
}

pub(super) fn retire_stable_id(
    module_id: &str,
    issue_id: &str,
    fix_candidate_id: &str,
    candidate_commit: &str,
    candidate_tree: &str,
) -> String {
    let identity = serde_json::json!({
        "module_id": module_id,
        "issue_id": issue_id,
        "fix_candidate_id": fix_candidate_id,
        "candidate_commit": candidate_commit,
        "candidate_tree": candidate_tree
    });
    format!(
        "candidate-{}",
        sha256(&canonical(&identity))
            .strip_prefix("sha256:")
            .unwrap_or_else(|| fail("RETIRE_STABLE_ID_INVALID"))
    )
}

pub(super) fn retire_write_bytes(root: &Path, target: &Path, bytes: &[u8]) {
    assert_no_symlink_components(root, target, "retire_archive_file");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .unwrap_or_else(|_| fail("RETIRE_ARCHIVE_WRITE_FAILED"));
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .unwrap_or_else(|_| fail("RETIRE_ARCHIVE_WRITE_FAILED"));
}

pub(super) fn retire_read_archive_file(root: &Path, target: &Path) -> Vec<u8> {
    assert_no_symlink_components(root, target, "retire_archive_file");
    let metadata = fs::symlink_metadata(target).unwrap_or_else(|_| fail("RETIRE_ARCHIVE_PARTIAL"));
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        fail("RETIRE_ARCHIVE_PARTIAL");
    }
    fs::read(target).unwrap_or_else(|_| fail("RETIRE_ARCHIVE_PARTIAL"))
}

pub(super) fn retire_manifest(
    module_id: &str,
    issue_id: &str,
    fix_candidate_id: &str,
    candidate_commit: &str,
    candidate_tree: &str,
    stable_id: &str,
    snapshots: &[RetireRecordSnapshot],
) -> Value {
    serde_json::json!({
        "schema_version": 1,
        "stable_id": stable_id,
        "module_id": module_id,
        "issue_id": issue_id,
        "fix_candidate_id": fix_candidate_id,
        "candidate_commit": candidate_commit,
        "candidate_tree": candidate_tree,
        "records": snapshots.iter().map(|snapshot| serde_json::json!({
            "kind": snapshot.kind,
            "file": snapshot.file_name,
            "source": snapshot.source.file_name().and_then(|name| name.to_str()).unwrap_or_else(|| fail("RETIRE_ARCHIVE_MANIFEST_INVALID")),
            "sha256": digest_bytes(&snapshot.bytes),
            "byte_length": snapshot.bytes.len()
        })).collect::<Vec<_>>()
    })
}

pub(super) fn retire_validate_archive(
    root: &Path,
    archive: &Path,
    module_id: &str,
    current_issue_id: &str,
) -> (String, String, String, String, String, Vec<Vec<u8>>) {
    assert_no_symlink_components(root, archive, "retire_archive");
    let metadata = fs::symlink_metadata(archive).unwrap_or_else(|_| fail("RETIRE_ARCHIVE_PARTIAL"));
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        fail("RETIRE_ARCHIVE_PARTIAL");
    }
    let manifest_path = archive.join("manifest.json");
    let manifest_bytes = retire_read_archive_file(root, &manifest_path);
    let manifest: Value =
        serde_json::from_slice(&manifest_bytes).unwrap_or_else(|_| fail("RETIRE_ARCHIVE_PARTIAL"));
    if manifest.get("schema_version").and_then(Value::as_u64) != Some(1)
        || manifest.get("module_id").and_then(Value::as_str) != Some(module_id)
    {
        fail("RETIRE_ARCHIVE_CONFLICT");
    }
    let stable_id = producer_string(&manifest, "/stable_id", "RETIRE_ARCHIVE_PARTIAL");
    if archive.file_name().and_then(|name| name.to_str()) != Some(stable_id.as_str()) {
        fail("RETIRE_ARCHIVE_CONFLICT");
    }
    let issue_id = producer_string(&manifest, "/issue_id", "RETIRE_ARCHIVE_PARTIAL");
    if issue_id == current_issue_id {
        fail("RETIRE_CURRENT_ISSUE_PROTECTED");
    }
    let fix_candidate_id =
        producer_string(&manifest, "/fix_candidate_id", "RETIRE_ARCHIVE_PARTIAL");
    let candidate_commit =
        producer_string(&manifest, "/candidate_commit", "RETIRE_ARCHIVE_PARTIAL");
    let candidate_tree = producer_string(&manifest, "/candidate_tree", "RETIRE_ARCHIVE_PARTIAL");
    retire_verify_candidate_identity(root, &candidate_commit, &candidate_tree);
    if retire_stable_id(
        module_id,
        &issue_id,
        &fix_candidate_id,
        &candidate_commit,
        &candidate_tree,
    ) != stable_id
    {
        fail("RETIRE_ARCHIVE_CONFLICT");
    }
    let entries = manifest
        .get("records")
        .and_then(Value::as_array)
        .filter(|entries| entries.len() == 2)
        .unwrap_or_else(|| fail("RETIRE_ARCHIVE_PARTIAL"));
    let expected = [
        ("fix-candidate-record", "fix-candidate-record.json"),
        (
            "pre-review-validation-record",
            "pre-review-validation-record.json",
        ),
    ];
    let mut archived_bytes = Vec::with_capacity(2);
    for (kind, file_name) in expected {
        let entry = entries
            .iter()
            .find(|entry| {
                entry.get("kind").and_then(Value::as_str) == Some(kind)
                    && entry.get("file").and_then(Value::as_str) == Some(file_name)
            })
            .unwrap_or_else(|| fail("RETIRE_ARCHIVE_PARTIAL"));
        let bytes = retire_read_archive_file(root, &archive.join(file_name));
        if entry.get("sha256").and_then(Value::as_str) != Some(digest_bytes(&bytes).as_str())
            || entry.get("byte_length").and_then(Value::as_u64) != Some(bytes.len() as u64)
        {
            fail("RETIRE_ARCHIVE_CONFLICT");
        }
        let record: Value =
            serde_json::from_slice(&bytes).unwrap_or_else(|_| fail("RETIRE_ARCHIVE_CONFLICT"));
        if producer_string(&record, "/module_id", "RETIRE_ARCHIVE_CONFLICT") != module_id
            || producer_issue(&record, "/issue_id", "RETIRE_ARCHIVE_CONFLICT") != issue_id
        {
            fail("RETIRE_ARCHIVE_CONFLICT");
        }
        if kind == "fix-candidate-record"
            && (producer_string(&record, "/fix_candidate_id", "RETIRE_ARCHIVE_CONFLICT")
                != fix_candidate_id
                || producer_string(&record, "/head_commit", "RETIRE_ARCHIVE_CONFLICT")
                    != candidate_commit
                || producer_string(&record, "/tree_hash", "RETIRE_ARCHIVE_CONFLICT")
                    != candidate_tree)
        {
            fail("RETIRE_ARCHIVE_CONFLICT");
        }
        if kind == "pre-review-validation-record"
            && (producer_string(&record, "/fix_candidate_id", "RETIRE_ARCHIVE_CONFLICT")
                != fix_candidate_id
                || producer_string(&record, "/candidate_commit", "RETIRE_ARCHIVE_CONFLICT")
                    != candidate_commit
                || producer_string(&record, "/candidate_tree_hash", "RETIRE_ARCHIVE_CONFLICT")
                    != candidate_tree)
        {
            fail("RETIRE_ARCHIVE_CONFLICT");
        }
        archived_bytes.push(bytes);
    }
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(archive).unwrap_or_else(|_| fail("RETIRE_ARCHIVE_PARTIAL")) {
        let entry = entry.unwrap_or_else(|_| fail("RETIRE_ARCHIVE_PARTIAL"));
        let name = entry.file_name().to_string_lossy().to_string();
        let metadata =
            fs::symlink_metadata(entry.path()).unwrap_or_else(|_| fail("RETIRE_ARCHIVE_PARTIAL"));
        if metadata.file_type().is_symlink() || !metadata.is_file() || !names.insert(name) {
            fail("RETIRE_ARCHIVE_PARTIAL");
        }
    }
    if names
        != BTreeSet::from([
            "fix-candidate-record.json".to_string(),
            "manifest.json".to_string(),
            "pre-review-validation-record.json".to_string(),
        ])
    {
        fail("RETIRE_ARCHIVE_PARTIAL");
    }
    (
        stable_id,
        issue_id,
        fix_candidate_id,
        candidate_commit,
        candidate_tree,
        archived_bytes,
    )
}

pub(super) fn retire_find_existing_archive(
    root: &Path,
    records_root: &Path,
    module_id: &str,
    current_issue_id: &str,
) -> Option<PathBuf> {
    let module_root = records_root.join("rejected").join(module_id);
    assert_no_symlink_components(root, &module_root, "retire_archive");
    let metadata = match fs::symlink_metadata(&module_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(_) => fail("RETIRE_ARCHIVE_PARTIAL"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        fail("RETIRE_ARCHIVE_PARTIAL");
    }
    let mut matches = Vec::new();
    for entry in fs::read_dir(&module_root).unwrap_or_else(|_| fail("RETIRE_ARCHIVE_PARTIAL")) {
        let entry = entry.unwrap_or_else(|_| fail("RETIRE_ARCHIVE_PARTIAL"));
        let path = entry.path();
        let metadata =
            fs::symlink_metadata(&path).unwrap_or_else(|_| fail("RETIRE_ARCHIVE_PARTIAL"));
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            fail("RETIRE_ARCHIVE_PARTIAL");
        }
        let (_, issue_id, _, _, _, _) =
            retire_validate_archive(root, &path, module_id, current_issue_id);
        if issue_id != current_issue_id {
            matches.push(path);
        }
    }
    if matches.len() > 1 {
        fail("RETIRE_ARCHIVE_SELECTION_AMBIGUOUS");
    }
    matches.into_iter().next()
}

pub(super) fn retire_remove_matching_sources(snapshots: &[RetireRecordSnapshot], archived_bytes: &[Vec<u8>]) {
    for (snapshot, archived) in snapshots.iter().zip(archived_bytes.iter()) {
        match fs::symlink_metadata(&snapshot.source) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    fail("RETIRE_SOURCE_CONFLICT");
                }
                let current =
                    fs::read(&snapshot.source).unwrap_or_else(|_| fail("RETIRE_SOURCE_CONFLICT"));
                if current != *archived {
                    fail("RETIRE_SOURCE_CONFLICT");
                }
                fs::remove_file(&snapshot.source)
                    .unwrap_or_else(|_| fail("RETIRE_SOURCE_REMOVE_FAILED"));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => fail("RETIRE_SOURCE_CONFLICT"),
        }
    }
    if let Some(parent) = snapshots
        .first()
        .and_then(|snapshot| snapshot.source.parent())
    {
        if let Ok(file) = OpenOptions::new().read(true).open(parent) {
            let _ = file.sync_all();
        }
    }
}

pub(super) fn retire_lifecycle_records(root: &Path, module_id: &str, current_issue_id: &str) {
    assert_project_root_safe(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    assert_identifier(current_issue_id, "INVALID_ISSUE_ID");
    let _producer_lock = producer_lock(root);
    let project = read_project(root);
    let module_exists = project
        .get("modules")
        .and_then(Value::as_array)
        .is_some_and(|modules| {
            modules
                .iter()
                .any(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        });
    if !module_exists {
        fail(format!("UNKNOWN_MODULE:{}", module_id));
    }
    let records_root = root.join(".appsdk").join("records");
    let candidate_path = records_root.join(module_record_name("fix-candidate-record", module_id));
    let validation_path = records_root.join(module_record_name(
        "pre-review-validation-record",
        module_id,
    ));
    let candidate_present = fs::symlink_metadata(&candidate_path).is_ok();
    let validation_present = fs::symlink_metadata(&validation_path).is_ok();
    if candidate_present && validation_present {
        let snapshots = vec![
            retire_record_snapshot(
                root,
                &records_root,
                module_id,
                "fix-candidate-record",
                "fix-candidate-record.json",
            ),
            retire_record_snapshot(
                root,
                &records_root,
                module_id,
                "pre-review-validation-record",
                "pre-review-validation-record.json",
            ),
        ];
        let candidate = &snapshots[0].value;
        let validation = &snapshots[1].value;
        let candidate_issue =
            producer_issue(candidate, "/issue_id", "RETIRE_RECORD_BINDING_MISMATCH");
        let validation_issue =
            producer_issue(validation, "/issue_id", "RETIRE_RECORD_BINDING_MISMATCH");
        let candidate_module =
            producer_string(candidate, "/module_id", "RETIRE_RECORD_BINDING_MISMATCH");
        let validation_module =
            producer_string(validation, "/module_id", "RETIRE_RECORD_BINDING_MISMATCH");
        let fix_candidate_id = producer_string(
            candidate,
            "/fix_candidate_id",
            "RETIRE_RECORD_BINDING_MISMATCH",
        );
        let validation_candidate_id = producer_string(
            validation,
            "/fix_candidate_id",
            "RETIRE_RECORD_BINDING_MISMATCH",
        );
        let candidate_commit =
            producer_string(candidate, "/head_commit", "RETIRE_RECORD_BINDING_MISMATCH");
        let validation_commit = producer_string(
            validation,
            "/candidate_commit",
            "RETIRE_RECORD_BINDING_MISMATCH",
        );
        let candidate_tree =
            producer_string(candidate, "/tree_hash", "RETIRE_RECORD_BINDING_MISMATCH");
        let validation_tree = producer_string(
            validation,
            "/candidate_tree_hash",
            "RETIRE_RECORD_BINDING_MISMATCH",
        );
        if candidate_module != module_id
            || validation_module != module_id
            || candidate_issue != validation_issue
            || candidate_issue == current_issue_id
            || candidate_issue.is_empty()
            || fix_candidate_id != validation_candidate_id
            || candidate_commit != validation_commit
            || candidate_tree != validation_tree
        {
            if candidate_issue == current_issue_id || validation_issue == current_issue_id {
                fail("RETIRE_CURRENT_ISSUE_PROTECTED");
            }
            fail("RETIRE_RECORD_BINDING_MISMATCH");
        }
        retire_verify_candidate_identity(root, &candidate_commit, &candidate_tree);
        let stable_id = retire_stable_id(
            module_id,
            &candidate_issue,
            &fix_candidate_id,
            &candidate_commit,
            &candidate_tree,
        );
        let rejected_root = records_root.join("rejected").join(module_id);
        let archive = rejected_root.join(&stable_id);
        let staging = rejected_root.join(format!(".{}.staging", stable_id));
        assert_no_symlink_components(root, &rejected_root, "retire_archive");
        fs::create_dir_all(&rejected_root).unwrap_or_else(|_| fail("RETIRE_ARCHIVE_CREATE_FAILED"));
        assert_no_symlink_components(root, &archive, "retire_archive");
        assert_no_symlink_components(root, &staging, "retire_archive");
        if fs::symlink_metadata(&archive).is_ok() {
            retire_validate_reentry_worktree(
                root,
                &[
                    candidate_path.clone(),
                    validation_path.clone(),
                    archive.join("manifest.json"),
                    archive.join("fix-candidate-record.json"),
                    archive.join("pre-review-validation-record.json"),
                ],
            );
            let (_, _, _, _, _, archived_bytes) =
                retire_validate_archive(root, &archive, module_id, current_issue_id);
            retire_remove_matching_sources(&snapshots, &archived_bytes);
            println!(
                "{}",
                serde_json::json!({"ok":true,"module_id":module_id,"issue_id":current_issue_id,"stable_id":stable_id,"retired":false,"reused":true})
            );
            return;
        }
        retire_validate_reentry_worktree(root, &[candidate_path.clone(), validation_path.clone()]);
        if fs::symlink_metadata(&staging).is_ok() {
            fail("RETIRE_ARCHIVE_PARTIAL");
        }
        fs::create_dir(&staging).unwrap_or_else(|_| fail("RETIRE_ARCHIVE_CREATE_FAILED"));
        let manifest = retire_manifest(
            module_id,
            &candidate_issue,
            &fix_candidate_id,
            &candidate_commit,
            &candidate_tree,
            &stable_id,
            &snapshots,
        );
        retire_write_bytes(
            root,
            &staging.join(snapshots[0].file_name),
            &snapshots[0].bytes,
        );
        retire_write_bytes(
            root,
            &staging.join(snapshots[1].file_name),
            &snapshots[1].bytes,
        );
        let manifest_bytes = (serde_json::to_string_pretty(&manifest).unwrap() + "\n").into_bytes();
        retire_write_bytes(root, &staging.join("manifest.json"), &manifest_bytes);
        if let Ok(file) = OpenOptions::new().read(true).open(&staging) {
            let _ = file.sync_all();
        }
        if fs::symlink_metadata(&archive).is_ok() {
            fail("RETIRE_ARCHIVE_CONFLICT");
        }
        fs::rename(&staging, &archive).unwrap_or_else(|_| fail("RETIRE_ARCHIVE_COMMIT_FAILED"));
        if let Some(parent) = archive.parent() {
            if let Ok(file) = OpenOptions::new().read(true).open(parent) {
                let _ = file.sync_all();
            }
        }
        retire_remove_matching_sources(
            &snapshots,
            &[snapshots[0].bytes.clone(), snapshots[1].bytes.clone()],
        );
        println!(
            "{}",
            serde_json::json!({"ok":true,"module_id":module_id,"issue_id":current_issue_id,"stable_id":stable_id,"retired":true,"reused":false})
        );
        return;
    }
    if candidate_present || validation_present {
        retire_validate_worktree(root);
        fail("RETIRE_RECORD_SET_INCOMPLETE");
    }
    let archive = retire_find_existing_archive(root, &records_root, module_id, current_issue_id)
        .unwrap_or_else(|| fail("RETIRE_RECORD_SET_INCOMPLETE"));
    retire_validate_reentry_worktree(
        root,
        &[
            candidate_path.clone(),
            validation_path.clone(),
            archive.join("manifest.json"),
            archive.join("fix-candidate-record.json"),
            archive.join("pre-review-validation-record.json"),
        ],
    );
    let (stable_id, _, _, _, _, archived_bytes) =
        retire_validate_archive(root, &archive, module_id, current_issue_id);
    let snapshots = vec![
        RetireRecordSnapshot {
            kind: "fix_candidate",
            file_name: "fix-candidate-record.json",
            source: candidate_path,
            bytes: Vec::new(),
            value: Value::Null,
        },
        RetireRecordSnapshot {
            kind: "pre_review_validation",
            file_name: "pre-review-validation-record.json",
            source: validation_path,
            bytes: Vec::new(),
            value: Value::Null,
        },
    ];
    retire_remove_matching_sources(&snapshots, &archived_bytes);
    println!(
        "{}",
        serde_json::json!({"ok":true,"module_id":module_id,"issue_id":current_issue_id,"stable_id":stable_id,"retired":false,"reused":true})
    );
}

pub(super) fn assert_version(value: &str, error: &str) {
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

pub(super) fn canonical_record_contract(relative: &str) -> Value {
    let content = SDK_BUNDLE_RESOURCES
        .iter()
        .find(|(path, class, _)| *path == relative && *class == "contracts")
        .map(|(_, _, content)| *content)
        .unwrap_or_else(|| fail("NON_CANONICAL_RECORD_CONTRACT_SET"));
    serde_json::from_str(content).unwrap_or_else(|_| fail("INVALID_CANONICAL_RECORD_CONTRACT"))
}

pub(super) fn schema_array_contains(values: &Value, expected: &Value) -> bool {
    values
        .as_array()
        .is_some_and(|entries| entries.iter().any(|entry| entry == expected))
}

pub(super) fn schema_required_names<'a>(schema: &'a Value, path: &str) -> &'a Vec<Value> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail(format!("INVALID_DECLARED_RECORD_CONTRACT:{}", path)))
}

pub(super) fn assert_schema_property_compatible(canonical: &Value, declared: &Value, path: &str) {
    if !canonical.is_object() || !declared.is_object() {
        fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{}", path));
    }
    for key in ["if", "then", "not"] {
        let Some(expected) = canonical.get(key) else {
            continue;
        };
        let actual = declared
            .get(key)
            .unwrap_or_else(|| fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{path}/{key}")));
        assert_schema_property_compatible(expected, actual, &format!("{path}/{key}"));
    }
    if let Some(expected_all_of) = canonical.get("allOf") {
        let Some(expected_rules) = expected_all_of.as_array() else {
            fail(format!("INVALID_CANONICAL_RECORD_CONTRACT:{}", path));
        };
        let Some(actual_rules) = declared.get("allOf").and_then(Value::as_array) else {
            fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{path}/allOf"));
        };
        for (index, expected_rule) in expected_rules.iter().enumerate() {
            let Some(actual_rule) = actual_rules.get(index) else {
                fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{path}/allOf"));
            };
            assert_schema_property_compatible(
                expected_rule,
                actual_rule,
                &format!("{path}/allOf/{index}"),
            );
        }
    }
    for key in [
        "type",
        "const",
        "minimum",
        "maximum",
        "minItems",
        "maxItems",
        "minLength",
        "maxLength",
        "pattern",
        "uniqueItems",
    ] {
        let Some(expected) = canonical.get(key) else {
            continue;
        };
        let Some(actual) = declared.get(key) else {
            fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{}", path));
        };
        let compatible = match key {
            "minimum" | "minItems" | "minLength" => actual
                .as_f64()
                .is_some_and(|value| expected.as_f64().is_some_and(|minimum| value >= minimum)),
            "maximum" | "maxItems" | "maxLength" => actual
                .as_f64()
                .is_some_and(|value| expected.as_f64().is_some_and(|maximum| value <= maximum)),
            _ => actual == expected,
        };
        if !compatible {
            fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{}", path));
        }
    }
    if let Some(expected_enum) = canonical.get("enum") {
        let Some(actual_enum) = declared.get("enum") else {
            fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{}", path));
        };
        let Some(expected_values) = expected_enum.as_array() else {
            fail(format!("INVALID_CANONICAL_RECORD_CONTRACT:{}", path));
        };
        let Some(actual_values) = actual_enum.as_array() else {
            fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{}", path));
        };
        if actual_values.is_empty()
            || actual_values
                .iter()
                .any(|value| !expected_values.iter().any(|entry| entry == value))
        {
            fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{}", path));
        }
    }
    if let Some(expected_items) = canonical.get("items") {
        let actual_items = declared
            .get("items")
            .unwrap_or_else(|| fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{}", path)));
        assert_schema_property_compatible(expected_items, actual_items, &format!("{path}/items"));
    }
    if let Some(expected_props) = canonical.get("properties").and_then(Value::as_object) {
        let actual_props = declared
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or_else(|| fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{}", path)));
        let expected_required = canonical
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (name, expected_property) in expected_props {
            let Some(actual_property) = actual_props.get(name) else {
                if expected_required
                    .iter()
                    .any(|required| required.as_str() == Some(name.as_str()))
                {
                    fail(format!(
                        "DECLARED_RECORD_CONTRACT_MISMATCH:{path}/properties/{name}"
                    ));
                }
                continue;
            };
            assert_schema_property_compatible(
                expected_property,
                actual_property,
                &format!("{path}/properties/{name}"),
            );
        }
    }
    if let Some(expected_required) = canonical.get("required") {
        let actual_required = declared
            .get("required")
            .unwrap_or_else(|| fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{}", path)));
        for required in expected_required
            .as_array()
            .unwrap_or_else(|| fail(format!("INVALID_CANONICAL_RECORD_CONTRACT:{}", path)))
        {
            if !schema_array_contains(actual_required, required) {
                fail(format!("DECLARED_RECORD_CONTRACT_MISMATCH:{}", path));
            }
        }
    }
}

pub(super) fn assert_record_schema_minimum(relative: &str, declared: &Value) {
    let canonical = canonical_record_contract(relative);
    let mut canonical_root = serde_json::Map::new();
    let mut declared_root = serde_json::Map::new();
    for key in ["if", "then", "not", "allOf"] {
        let Some(expected) = canonical.get(key) else {
            continue;
        };
        let actual = declared.get(key).unwrap_or_else(|| {
            fail(format!(
                "DECLARED_RECORD_CONTRACT_MISMATCH:{relative}/{key}"
            ))
        });
        canonical_root.insert(key.into(), expected.clone());
        declared_root.insert(key.into(), actual.clone());
    }
    if !canonical_root.is_empty() {
        assert_schema_property_compatible(
            &Value::Object(canonical_root),
            &Value::Object(declared_root),
            relative,
        );
    }
    let canonical_required = schema_required_names(&canonical, relative);
    let declared_required = schema_required_names(declared, relative);
    let declared_properties = declared
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("INVALID_DECLARED_RECORD_CONTRACT"));
    let canonical_properties = canonical
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("INVALID_CANONICAL_RECORD_CONTRACT"));
    for required in canonical_required {
        let name = required
            .as_str()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| fail("INVALID_CANONICAL_RECORD_CONTRACT"));
        let compatibility_exception = relative == "contracts/records/promotion-record.schema.json"
            && name == "bug_closure_verified";
        let required_present =
            schema_array_contains(&Value::Array(declared_required.clone()), required);
        let declared_property = declared_properties.get(name);
        if (!required_present || declared_property.is_none()) && compatibility_exception {
            continue;
        }
        if !required_present || declared_property.is_none() {
            fail(format!(
                "DECLARED_RECORD_CONTRACT_MISMATCH:{relative}:{name}"
            ));
        }
        let canonical_property = canonical_properties
            .get(name)
            .unwrap_or_else(|| fail("INVALID_CANONICAL_RECORD_CONTRACT"));
        assert_schema_property_compatible(
            canonical_property,
            declared_property.unwrap(),
            &format!("{relative}/properties/{name}"),
        );
    }
}

pub(super) fn assert_declared_contracts(root: &Path, project: &Value) {
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
    if !canonical_project {
        fail("NON_CANONICAL_GOVERNANCE_CONTRACT");
    }
    let declared_records = project
        .pointer("/governance/record_contracts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/governance/record_contracts"));
    if declared_records.len() != CANONICAL_RECORD_CONTRACTS.len()
        || CANONICAL_RECORD_CONTRACTS.iter().any(|path| {
            !declared_records
                .iter()
                .any(|value| value.as_str() == Some(*path))
        })
    {
        fail("NON_CANONICAL_RECORD_CONTRACT_SET");
    }
    let zone_text =
        fs::read_to_string(&zone).unwrap_or_else(|_| fail("DECLARED_ZONE_CONTRACT_MISSING"));
    let zone_value: Value =
        serde_json::from_str(&zone_text).unwrap_or_else(|_| fail("INVALID_DECLARED_ZONE_CONTRACT"));
    let canonical_zone: Value = serde_json::from_str(CANONICAL_ZONE_TRANSITION_CONTRACT)
        .unwrap_or_else(|_| fail("INVALID_CANONICAL_ZONE_CONTRACT"));
    if zone_value.get("zones") != canonical_zone.get("zones") {
        fail("INVALID_DECLARED_ZONE_CONTRACT");
    }
    let declared_transitions = zone_value
        .get("transitions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_DECLARED_ZONE_CONTRACT"));
    let canonical_transitions = canonical_zone
        .get("transitions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_CANONICAL_ZONE_CONTRACT"));
    let declared_transition_keys = declared_transitions
        .iter()
        .map(|transition| {
            transition
                .get("from")
                .and_then(Value::as_str)
                .zip(transition.get("to").and_then(Value::as_str))
                .unwrap_or_else(|| fail("INVALID_DECLARED_ZONE_CONTRACT"))
        })
        .collect::<Vec<_>>();
    let mut unique_transition_keys = BTreeSet::new();
    if declared_transition_keys
        .iter()
        .any(|key| !unique_transition_keys.insert(key))
    {
        fail("INVALID_DECLARED_ZONE_CONTRACT");
    }
    let canonical_transition_keys = canonical_transitions
        .iter()
        .map(|transition| {
            transition
                .get("from")
                .and_then(Value::as_str)
                .zip(transition.get("to").and_then(Value::as_str))
                .unwrap_or_else(|| fail("INVALID_CANONICAL_ZONE_CONTRACT"))
        })
        .collect::<Vec<_>>();
    let mut unique_canonical_keys = BTreeSet::new();
    if canonical_transition_keys
        .iter()
        .any(|key| !unique_canonical_keys.insert(key))
    {
        fail("INVALID_CANONICAL_ZONE_CONTRACT");
    }
    if declared_transition_keys.len() != canonical_transition_keys.len()
        || canonical_transition_keys
            .iter()
            .any(|key| !declared_transition_keys.contains(key))
    {
        fail("INVALID_DECLARED_ZONE_CONTRACT");
    }
    for transition in declared_transitions {
        let object = transition
            .as_object()
            .unwrap_or_else(|| fail("INVALID_DECLARED_ZONE_CONTRACT"));
        for field in ["from", "to", "owner"] {
            if object
                .get(field)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                fail("INVALID_DECLARED_ZONE_CONTRACT");
            }
        }
        if object.get("allowed").and_then(Value::as_bool).is_none()
            || object
                .get("runtime_allowed")
                .and_then(Value::as_bool)
                .is_none()
            || object
                .get("artifact_required")
                .and_then(Value::as_bool)
                .is_none()
            || object
                .get("requirements")
                .and_then(Value::as_array)
                .is_none()
            || object
                .get("record_required")
                .and_then(Value::as_array)
                .is_none()
        {
            fail("INVALID_DECLARED_ZONE_CONTRACT");
        }
        for field in ["requirements", "record_required"] {
            if object[field]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value.as_str().is_none())
            {
                fail("INVALID_DECLARED_ZONE_CONTRACT");
            }
        }
    }
    for expected in canonical_transitions {
        let key = expected
            .get("from")
            .and_then(Value::as_str)
            .zip(expected.get("to").and_then(Value::as_str))
            .unwrap_or_else(|| fail("INVALID_CANONICAL_ZONE_CONTRACT"));
        let actual = declared_transitions
            .iter()
            .find(|transition| {
                transition
                    .get("from")
                    .and_then(Value::as_str)
                    .zip(transition.get("to").and_then(Value::as_str))
                    .is_some_and(|actual_key| actual_key == key)
            })
            .unwrap_or_else(|| fail("INVALID_DECLARED_ZONE_CONTRACT"));
        if actual != expected {
            fail("INVALID_DECLARED_ZONE_CONTRACT");
        }
    }
    if let Some(expected_forbidden) = canonical_zone.get("forbidden_runtime_edges") {
        let actual_forbidden = zone_value
            .get("forbidden_runtime_edges")
            .and_then(Value::as_array)
            .unwrap_or_else(|| fail("INVALID_DECLARED_ZONE_CONTRACT"));
        if !expected_forbidden
            .as_array()
            .unwrap_or_else(|| fail("INVALID_CANONICAL_ZONE_CONTRACT"))
            .iter()
            .all(|expected| actual_forbidden.iter().any(|actual| actual == expected))
        {
            fail("INVALID_DECLARED_ZONE_CONTRACT");
        }
    }
    let canonical_path = zone.with_file_name("zone-transition.manifest.json");
    if !canonical_path.exists() {
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
        let text =
            fs::read_to_string(path).unwrap_or_else(|_| fail("DECLARED_RECORD_CONTRACT_MISSING"));
        let value: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| fail("INVALID_DECLARED_RECORD_CONTRACT"));
        let schema = value
            .get("$schema")
            .and_then(Value::as_str)
            .filter(|schema| !schema.is_empty());
        let schema_id = value
            .get("$id")
            .and_then(Value::as_str)
            .filter(|schema_id| !schema_id.is_empty());
        let schema_type = value.get("type").and_then(Value::as_str);
        let properties = value.get("properties").and_then(Value::as_object);
        let required = value.get("required").and_then(Value::as_array);
        if schema.is_none()
            || schema_id.is_none()
            || schema_type != Some("object")
            || properties.is_none()
            || required.is_none_or(|values| {
                values.is_empty()
                    || values
                        .iter()
                        .any(|entry| entry.as_str().filter(|name| !name.is_empty()).is_none())
            })
        {
            fail("INVALID_DECLARED_RECORD_CONTRACT");
        }
        assert_record_schema_minimum(relative, &value);
    }
}

pub(super) fn assert_governance_maps(root: &Path) {
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
