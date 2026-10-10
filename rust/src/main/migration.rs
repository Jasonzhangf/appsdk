use super::*;

pub(super) fn init_collab_control_project(root: &Path) {
    assert_ordinary_init_canonical_project_main_tree(root, false);
    fs::create_dir_all(root).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    try_register_global_project(root);
    initialize_collab_peer(root);
    println!("initialized collab identity {}", root.display());
}

pub(super) fn new_project(root: &Path, register: bool) {
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
    fs::create_dir_all(root).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    let registration = register.then(|| reserve_global_project(root));
    ensure_governance_layout(root);
    write_project_scaffold(root);
    write_project_agent_contract(root);
    install_bundle_resources(root);
    write_current_sdk_lock(root);
    install_standard_template_reference(root);
    if let Some(registration) = registration {
        commit_global_project(registration);
    }
    if let Err(reason) = memory::initialize_project(root) {
        eprintln!("{}; optional project memory initialization skipped", reason);
    }
    println!("created {}", root.display());
    println!("next appsdk guide compile");
    println!("then appsdk guide init --task <task-id> --mode <develop|debug> --module <module-id>");
}

pub(super) fn reset_staging_scaffold(root: &Path, transaction_dir: &Path, transaction_id: &str) {
    let expected_transaction_dir = reset_transaction_dir(root);
    if transaction_dir != expected_transaction_dir
        || transaction_id.trim().is_empty()
        || !root.is_absolute()
        || !transaction_dir.is_absolute()
    {
        fail("GOVERNANCE_RESET_STAGING_AUTH_FAILED");
    }
    let marker_path = transaction_dir.join("marker.json");
    if fs::symlink_metadata(transaction_dir)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(true)
        || fs::symlink_metadata(&marker_path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(true)
    {
        fail("GOVERNANCE_RESET_STAGING_AUTH_FAILED");
    }
    let marker: Value = serde_json::from_str(
        &fs::read_to_string(&marker_path)
            .unwrap_or_else(|_| fail("GOVERNANCE_RESET_STAGING_AUTH_FAILED")),
    )
    .unwrap_or_else(|_| fail("GOVERNANCE_RESET_STAGING_AUTH_FAILED"));
    let root_text = root.to_string_lossy();
    if marker.get("transaction_id").and_then(Value::as_str) != Some(transaction_id)
        || marker.get("root").and_then(Value::as_str) != Some(root_text.as_ref())
        || marker.get("phase").and_then(Value::as_str) != Some("building")
    {
        fail("GOVERNANCE_RESET_STAGING_AUTH_FAILED");
    }
    let project_path = root.join(".appsdk/project.json");
    if fs::symlink_metadata(&project_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:project");
    }
    let project_bytes = fs::read(&project_path)
        .unwrap_or_else(|_| fail("GOVERNANCE_RESET_PROJECT_CONTRACT_MISSING"));
    let project: Value = serde_json::from_slice(&project_bytes)
        .unwrap_or_else(|_| fail("GOVERNANCE_RESET_PROJECT_CONTRACT_INVALID"));
    let staging_root = transaction_dir.join("staging");
    new_project(&staging_root, false);
    let scaffold: Value = serde_json::from_str(
        &fs::read_to_string(staging_root.join(".appsdk/project.json"))
            .unwrap_or_else(|_| fail("GOVERNANCE_RESET_PROJECT_CONTRACT_INVALID")),
    )
    .unwrap_or_else(|_| fail("GOVERNANCE_RESET_PROJECT_CONTRACT_INVALID"));
    let staging_project = rebuild_fresh_project_contract(&project, &scaffold);
    let staging_project_bytes = if staging_project == project {
        project_bytes.clone()
    } else {
        let mut bytes = serde_json::to_vec_pretty(&staging_project)
            .unwrap_or_else(|_| fail("GOVERNANCE_RESET_PROJECT_CONTRACT_INVALID"));
        bytes.push(b'\n');
        bytes
    };
    let registry_path = root.join(".appsdk/maps/module-registry.json");
    assert_no_symlink_components(root, &registry_path, "reset_module_registry");
    if !fs::symlink_metadata(&registry_path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_RESET_MODULE_REGISTRY_INVALID");
    }
    let registry: Value = serde_json::from_str(
        &fs::read_to_string(&registry_path)
            .unwrap_or_else(|_| fail("GOVERNANCE_RESET_MODULE_REGISTRY_MISSING")),
    )
    .unwrap_or_else(|_| fail("GOVERNANCE_RESET_MODULE_REGISTRY_INVALID"));
    if registry.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail("GOVERNANCE_RESET_MODULE_REGISTRY_INVALID");
    }
    let registry_modules = registry
        .get("modules")
        .and_then(Value::as_array)
        .filter(|modules| !modules.is_empty())
        .unwrap_or_else(|| fail("GOVERNANCE_RESET_MODULE_REGISTRY_INVALID"));
    let project_modules = staging_project
        .get("modules")
        .and_then(Value::as_array)
        .filter(|modules| !modules.is_empty())
        .unwrap_or_else(|| fail("GOVERNANCE_RESET_CONTRACT_REQUIRED:modules"));
    let mut staging_registry = registry.clone();
    let mut staging_modules = Vec::with_capacity(project_modules.len());
    for module in project_modules {
        let module_id = module
            .get("module_id")
            .and_then(Value::as_str)
            .filter(|module_id| !module_id.is_empty())
            .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/modules"));
        let matches: Vec<&Value> = registry_modules
            .iter()
            .filter(|registered| {
                registered.get("module_id").and_then(Value::as_str) == Some(module_id)
            })
            .collect();
        if matches.len() > 1 {
            fail(format!(
                "GOVERNANCE_RESET_MODULE_REGISTRY_DUPLICATE:{module_id}"
            ));
        }
        let owner = module
            .get("source_owner")
            .and_then(Value::as_str)
            .filter(|owner| !owner.is_empty())
            .unwrap_or_else(|| {
                fail(format!(
                    "GOVERNANCE_RESET_MODULE_REGISTRY_OWNER:{module_id}"
                ))
            });
        let owned_paths = module
            .get("owned_paths")
            .and_then(Value::as_array)
            .filter(|paths| !paths.is_empty())
            .cloned()
            .unwrap_or_else(|| {
                fail(format!(
                    "GOVERNANCE_RESET_MODULE_REGISTRY_PATHS:{module_id}"
                ))
            });
        let default_forbidden_paths = matches
            .first()
            .and_then(|registered| registered.get("forbidden_paths"))
            .and_then(Value::as_array)
            .or_else(|| {
                registry_modules
                    .first()
                    .and_then(|registered| registered.get("forbidden_paths"))
                    .and_then(Value::as_array)
            });
        let forbidden_paths: Vec<Value> = default_forbidden_paths
            .into_iter()
            .flatten()
            .cloned()
            .filter(|forbidden| {
                let Some(forbidden) = forbidden.as_str() else {
                    return true;
                };
                !owned_paths
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|owned| registry_path_matches(forbidden, owned))
            })
            .collect();
        let verification_gates = matches
            .first()
            .and_then(|registered| registered.get("verification_gates"))
            .and_then(Value::as_array)
            .filter(|gates| !gates.is_empty())
            .cloned()
            .or_else(|| {
                module
                    .pointer("/regression/suite_id")
                    .and_then(Value::as_str)
                    .filter(|suite_id| !suite_id.is_empty())
                    .map(|suite_id| vec![Value::String(suite_id.to_string())])
            })
            .unwrap_or_default();
        let mut staging_module = matches
            .first()
            .map(|registered| (*registered).clone())
            .unwrap_or_else(|| serde_json::json!({}));
        staging_module["module_id"] = Value::String(module_id.to_string());
        staging_module["status"] = Value::String("active".into());
        staging_module["owner"] = Value::String(owner.to_string());
        staging_module["owned_paths"] = Value::Array(owned_paths);
        staging_module["forbidden_paths"] = Value::Array(forbidden_paths);
        staging_module["verification_gates"] = Value::Array(verification_gates);
        staging_modules.push(staging_module);
    }
    staging_registry["modules"] = Value::Array(staging_modules);
    // Fresh init resets the control-plane records and rebuildable projections.
    // The current scaffold is the reset baseline for SDK-owned fields; the
    // existing project contract contributes only project-owned identity,
    // ownership, build, and protection boundaries. Legacy SDK pins and witness
    // files are ignored. Validate the rebuilt current-format contract before
    // publishing the transaction.
    reset_transaction_write_bytes(
        transaction_dir,
        &staging_root.join(".appsdk/project.json"),
        &staging_project_bytes,
    )
    .unwrap_or_else(|error| fail(error));
    reset_transaction_write_json(
        transaction_dir,
        &staging_root.join(".appsdk/maps/module-registry.json"),
        &staging_registry,
    )
    .unwrap_or_else(|error| fail(error));
    // Validate against the freshly built SDK resources so stale legacy
    // projections do not prevent a valid contract from being checked.  The
    // transaction has not quarantined or published any project paths yet.
    assert_project_contract(&staging_root, &staging_project);
}

pub(super) fn rebuild_fresh_project_contract(project: &Value, scaffold: &Value) -> Value {
    if !project.is_object() {
        fail("INVALID_PROJECT_CONTRACT");
    }
    let project_object = project
        .as_object()
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT"));
    let project_id = project
        .get("project_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail("GOVERNANCE_RESET_CONTRACT_REQUIRED:project_id"));
    let access = project
        .get("access")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("GOVERNANCE_RESET_CONTRACT_REQUIRED:access"));
    if access
        .get("protected_paths")
        .and_then(Value::as_array)
        .is_none()
    {
        fail("GOVERNANCE_RESET_CONTRACT_REQUIRED:access.protected_paths");
    }
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("GOVERNANCE_RESET_CONTRACT_REQUIRED:modules"));

    // The current scaffold is the reset baseline. Legacy SDK shape, version,
    // guidance, lifecycle machinery, record contracts, and other rebuildable
    // projections are deliberately not copied. Only project-owned boundaries
    // are overlaid onto the current baseline.
    let mut rebuilt = scaffold.clone();
    rebuilt["project_id"] = Value::String(project_id.to_string());
    let rebuilt_access = rebuilt
        .get_mut("access")
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| fail("GOVERNANCE_RESET_STAGING_CONTRACT_INVALID"));
    for (key, value) in access {
        rebuilt_access.insert(key.clone(), value.clone());
    }
    if let Some(stage) = project.pointer("/lifecycle/stage").and_then(Value::as_str) {
        rebuilt["lifecycle"]["stage"] = Value::String(stage.to_string());
    }
    if let Some(enabled) = project
        .pointer("/development_scenarios/enabled")
        .and_then(Value::as_array)
    {
        rebuilt["development_scenarios"]["enabled"] = Value::Array(enabled.clone());
    }
    let rebuilt_governance = rebuilt
        .get_mut("governance")
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| fail("GOVERNANCE_RESET_STAGING_CONTRACT_INVALID"));
    for key in [
        "playground_root",
        "active_root",
        "protected_root",
        "generated_root",
    ] {
        if let Some(value) = project
            .pointer(&format!("/governance/{key}"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            rebuilt_governance.insert(key.into(), Value::String(value.to_string()));
        }
    }
    for key in [
        "freeze_requirements",
        "promotion_requires",
        "runtime_forbidden_roots",
        "protected_kinds",
        "generated_kinds",
    ] {
        if let Some(value) = project.pointer(&format!("/governance/{key}")) {
            rebuilt_governance.insert(key.into(), value.clone());
        }
    }

    let module_template = scaffold
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| modules.first())
        .cloned()
        .unwrap_or_else(|| fail("GOVERNANCE_RESET_STAGING_CONTRACT_INVALID"));
    let mut rebuilt_modules = Vec::with_capacity(modules.len());
    for module in modules {
        let module = module
            .as_object()
            .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/modules"));
        let mut current = module_template.clone();
        let current_object = current
            .as_object_mut()
            .unwrap_or_else(|| fail("GOVERNANCE_RESET_STAGING_CONTRACT_INVALID"));
        for (key, value) in module {
            // version_base binds the old epoch to old Active artifacts. It is
            // rebuilt only when the new epoch actually needs a version move.
            if key != "version_base" {
                current_object.insert(key.clone(), value.clone());
            }
        }
        current_object
            .entry("dependency_modules")
            .or_insert_with(|| Value::Array(Vec::new()));
        rebuilt_modules.push(current);
    }
    rebuilt["modules"] = Value::Array(rebuilt_modules);

    for (key, value) in project_object {
        if matches!(
            key.as_str(),
            "schema_version"
                | "project_id"
                | "sdk"
                | "lifecycle"
                | "access"
                | "development_scenarios"
                | "guidance"
                | "governance"
                | "lifecycles"
                | "modules"
        ) {
            continue;
        }
        rebuilt[key] = value.clone();
    }
    rebuilt
}

pub(super) fn sdk_map_migration_root(root: &Path, step: &str) -> PathBuf {
    root.join(".appsdk").join("migrations").join(step)
}

pub(super) fn sdk_map_migration_entry<'a>(manifest: &'a Value, name: &str) -> &'a Value {
    manifest
        .get("maps")
        .and_then(Value::as_array)
        .and_then(|maps| {
            maps.iter()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
        })
        .unwrap_or_else(|| fail("INVALID_SDK_MAP_MIGRATION_MANIFEST"))
}

pub(super) fn valid_bundle_digest(digest: &str) -> bool {
    digest
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn sdk_map_migration_historical_target_authorized(
    declared: &Value,
    bundle_digest: &str,
    target_digest: &Value,
) -> bool {
    let Some(target_digest) = target_digest
        .as_str()
        .filter(|digest| valid_bundle_digest(digest))
    else {
        return false;
    };
    declared
        .get("historical_target_digests")
        .and_then(Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry.get("bundle_digest").and_then(Value::as_str) == Some(bundle_digest)
                    && entry.get("target_digest").and_then(Value::as_str) == Some(target_digest)
            })
        })
}

pub(super) fn migration_bundle_transition_digest(root: &Path, record: &Value) -> Option<String> {
    let record_bundle = record
        .get("bundle_digest")
        .and_then(Value::as_str)
        .filter(|digest| valid_bundle_digest(digest))?;
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
    let lock_bundle = lock
        .get("bundle_digest")
        .and_then(Value::as_str)
        .filter(|digest| valid_bundle_digest(digest))?;
    let lock_previous_bundles = lock
        .get("previous_bundle_digests")
        .and_then(Value::as_array)
        .map(|digests| {
            digests
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            lock.get("previous_bundle_digest")
                .and_then(Value::as_str)
                .into_iter()
                .map(str::to_owned)
                .collect()
        });
    let current_bundle = sdk_bundle_digest();
    if record_bundle == current_bundle {
        return None;
    }
    if lock_bundle == record_bundle
        || lock_previous_bundles
            .iter()
            .any(|known| known == record_bundle)
    {
        return Some(record_bundle.to_string());
    }
    None
}

pub(super) fn lock_migration_bundle_witnesses(lock: &Value) -> Vec<String> {
    if let Some(digests) = lock.get("previous_bundle_digests") {
        return digests
            .as_array()
            .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_DIGEST"))
            .iter()
            .map(|digest| {
                let digest = digest
                    .as_str()
                    .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_DIGEST"));
                if !valid_bundle_digest(digest) {
                    fail("INVALID_SDK_BUNDLE_DIGEST");
                }
                digest.to_string()
            })
            .collect();
    }
    if let Some(digest) = lock.get("previous_bundle_digest") {
        let digest = digest
            .as_str()
            .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_DIGEST"));
        if !valid_bundle_digest(digest) {
            fail("INVALID_SDK_BUNDLE_DIGEST");
        }
        return vec![digest.to_string()];
    }
    Vec::new()
}

pub(super) fn sdk_migration_bundle_witnesses(root: &Path) -> Vec<String> {
    let lock_path = root.join(".appsdk/sdk.lock");
    if !lock_path.is_file() {
        return Vec::new();
    }
    let lock: Value = serde_json::from_str(
        &fs::read_to_string(&lock_path).unwrap_or_else(|_| fail("INVALID_SDK_LOCK")),
    )
    .unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
    let lock_witnesses = lock_migration_bundle_witnesses(&lock);
    let lock_bundle = lock
        .get("bundle_digest")
        .and_then(Value::as_str)
        .filter(|digest| valid_bundle_digest(digest));
    let mut records = Vec::new();
    for step in SDK_MAP_MIGRATION_STEPS {
        let record_path = sdk_map_migration_root(root, step).join("record.json");
        if !record_path.is_file() {
            continue;
        }
        let record: Value = serde_json::from_str(
            &fs::read_to_string(&record_path)
                .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD")),
        )
        .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD"));
        if let Some(digest) = record
            .get("bundle_digest")
            .and_then(Value::as_str)
            .filter(|digest| valid_bundle_digest(digest))
        {
            if digest != sdk_bundle_digest() {
                records.push(digest.to_string());
            }
        }
    }
    if !records
        .iter()
        .any(|digest| Some(digest.as_str()) == lock_bundle)
        && !records
            .iter()
            .any(|digest| lock_witnesses.iter().any(|known| known == digest))
    {
        return Vec::new();
    }
    let mut witnesses = lock_witnesses;
    for digest in records {
        if !witnesses.iter().any(|known| known == &digest) {
            witnesses.push(digest);
        }
    }
    witnesses
}

pub(super) fn sdk_map_migration_manifest_versions(manifest: &Value) -> (&str, &str) {
    (
        manifest
            .get("source_version")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("INVALID_SDK_MAP_MIGRATION_MANIFEST")),
        manifest
            .get("target_version")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("INVALID_SDK_MAP_MIGRATION_MANIFEST")),
    )
}

pub(super) fn sdk_map_migration_target_content(manifest: &Value, name: &str) -> &'static str {
    let (_, target_version) = sdk_map_migration_manifest_versions(manifest);
    if target_version == SDK_VERSION {
        canonical_governance_map(name)
    } else {
        historical_governance_map(target_version, name)
    }
}

pub(super) fn sdk_map_migration_checks_live_target(step: &str) -> bool {
    let manifest = sdk_map_migration_manifest(step);
    let (_, target_version) = sdk_map_migration_manifest_versions(&manifest);
    target_version == SDK_VERSION
}

pub(super) fn sdk_historical_review_map_binding(
    root: &Path,
    module_id: &str,
    review: &Value,
    review_name: &str,
) -> bool {
    let bindings = [
        ("resource-map.json", "resource_map_hash"),
        ("function-map.json", "function_map_hash"),
        ("mainline-call-map.json", "mainline_call_map_hash"),
        ("verification-map.json", "verification_map_hash"),
    ];
    let review_id = record_str(review, "/review_id", review_name);
    SDK_MAP_MIGRATION_STEPS
        .iter()
        .filter(|step| !sdk_map_migration_checks_live_target(step))
        .any(|step| {
            if !sdk_map_migration_root(root, step)
                .join("record.json")
                .is_file()
            {
                return false;
            }
            let migration = assert_sdk_migration_record(root, step, false)
                .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
            let retained = ["frozen_reviews", "legacy_reconciled_reviews"]
                .iter()
                .any(|key| {
                    migration
                        .get(*key)
                        .and_then(Value::as_array)
                        .is_some_and(|reviews| {
                            reviews.iter().any(|entry| {
                                entry.get("module_id").and_then(Value::as_str) == Some(module_id)
                                    && entry.get("review_id").and_then(Value::as_str)
                                        == Some(review_id)
                            })
                        })
                });
            if !retained {
                return false;
            }
            let manifest = sdk_map_migration_manifest(step);
            bindings.iter().all(|(name, field)| {
                let entry = sdk_map_migration_entry(&manifest, name);
                record_str(review, &format!("/{field}"), review_name)
                    == record_str(entry, "/source_digest", "sdk-map-migration")
            })
        })
}

pub(super) fn assert_sdk_migration_record(
    root: &Path,
    step: &str,
    check_live_target: bool,
) -> Option<Value> {
    let migration_root = sdk_map_migration_root(root, step);
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
    let manifest = sdk_map_migration_manifest(step);
    let (source_version, target_version) = sdk_map_migration_manifest_versions(&manifest);
    if record.get("schema_version").and_then(Value::as_u64) != Some(1)
        || record.get("migration_id").and_then(Value::as_str)
            != manifest.get("migration_id").and_then(Value::as_str)
        || record.get("source_version").and_then(Value::as_str) != Some(source_version)
        || record.get("target_version").and_then(Value::as_str) != Some(target_version)
        || record
            .get("bundle_digest")
            .and_then(Value::as_str)
            .filter(|digest| valid_bundle_digest(digest))
            .is_none()
        || DateTime::parse_from_rfc3339(record_str(&record, "/created_at", "sdk-migration-record"))
            .is_err()
    {
        fail("INVALID_SDK_MIGRATION_RECORD");
    }
    let maps = record
        .get("maps")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
    if maps.len() != GOVERNANCE_MAP_NAMES.len() {
        fail("INVALID_SDK_MIGRATION_RECORD");
    }
    let record_bundle = record
        .get("bundle_digest")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
    let bundle_transition = migration_bundle_transition_digest(root, &record).is_some();
    for name in GOVERNANCE_MAP_NAMES {
        let declared = sdk_map_migration_entry(&manifest, name);
        let entry = maps
            .iter()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        let expected_snapshot = format!(".appsdk/migrations/{step}/maps/{}", name);
        let canonical_source = entry
            .get("canonical_source_digest")
            .or_else(|| entry.get("source_digest"))
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        let canonical_target = entry
            .get("canonical_target_digest")
            .or_else(|| entry.get("target_digest"))
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        let explicit_custom_source = entry
            .get("canonical_source_digest")
            .is_some_and(|value| !value.is_null());
        let explicit_custom_target = entry
            .get("canonical_target_digest")
            .is_some_and(|value| !value.is_null());
        let historical_target_authorized = bundle_transition
            && !sdk_map_migration_checks_live_target(step)
            && explicit_custom_source
            && explicit_custom_target
            && Some(canonical_source) == declared.get("source_digest")
            && sdk_map_migration_historical_target_authorized(
                declared,
                record_bundle,
                canonical_target,
            );
        let actual_source = entry
            .get("source_digest")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        let actual_target = entry
            .get("target_digest")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
        let actual_target_bound = if explicit_custom_target {
            actual_target == actual_source
                || historical_target_authorized
                || (bundle_transition
                    && Some(canonical_target) == entry.get("target_digest")
                    && canonical_target.as_str().is_some_and(valid_bundle_digest))
        } else {
            declared.get("target_digest").and_then(Value::as_str) == Some(actual_target)
        };
        if !actual_target_bound {
            fail(format!("SDK_MIGRATION_TARGET_MAP_MISMATCH:{}", name));
        }
        if entry
            .get("canonical_source_digest")
            .is_some_and(|value| !value.is_null() && Some(value) != declared.get("source_digest"))
            || (Some(canonical_target) != declared.get("target_digest")
                && explicit_custom_target
                && !(bundle_transition
                    && sdk_map_migration_checks_live_target(step)
                    && canonical_target.as_str().is_some_and(valid_bundle_digest)
                    && Some(canonical_target) != declared.get("target_digest"))
                && !(bundle_transition
                    && Some(canonical_target) == entry.get("target_digest")
                    && canonical_target.as_str().is_some_and(valid_bundle_digest))
                && !historical_target_authorized)
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
        if check_live_target {
            let live_digest = file_sha256(&root.join(".appsdk/maps").join(name), "governance_map");
            let current_target = record_str(declared, "/target_digest", "sdk-map-migration");
            let current_target_is_authorized = bundle_transition
                && live_digest == current_target
                && explicit_custom_source
                && explicit_custom_target
                && Some(canonical_source) == declared.get("source_digest")
                && Some(canonical_target) == entry.get("target_digest");
            if live_digest != record_str(entry, "/target_digest", "sdk-migration-map")
                && !current_target_is_authorized
            {
                fail(format!("SDK_MIGRATION_TARGET_MAP_MISMATCH:{}", name));
            }
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

pub(super) fn install_governance_maps(root: &Path, step: &str, force: bool) {
    let record_path = sdk_map_migration_root(root, step).join("record.json");
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
    let manifest = sdk_map_migration_manifest(step);
    for name in GOVERNANCE_MAP_NAMES {
        let target = root.join(".appsdk/maps").join(name);
        atomic_write_bytes(
            &target,
            sdk_map_migration_target_content(&manifest, name).as_bytes(),
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

pub(super) fn migrate_governance_maps(root: &Path, project: &Value, step: &str) {
    let migration_root = sdk_map_migration_root(root, step);
    if migration_root.join("record.json").is_file() {
        let manifest = sdk_map_migration_manifest(step);
        let (_, target_version) = sdk_map_migration_manifest_versions(&manifest);
        if target_version != SDK_VERSION {
            let _ = assert_sdk_migration_record(root, step, false);
            return;
        }
        let record: Value = serde_json::from_str(
            &fs::read_to_string(migration_root.join("record.json"))
                .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD")),
        )
        .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD"));
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
        install_governance_maps(root, step, source_maps);
        let _ = assert_sdk_migration_record(root, step, true);
        return;
    }
    let manifest = sdk_map_migration_manifest(step);
    let (source_version, target_version) = sdk_map_migration_manifest_versions(&manifest);
    let canonical_source_matches = GOVERNANCE_MAP_NAMES.iter().all(|name| {
        file_sha256(&root.join(".appsdk/maps").join(name), "governance_map")
            == record_str(
                sdk_map_migration_entry(&manifest, name),
                "/source_digest",
                "sdk-map-migration",
            )
    });
    let current_target_matches = GOVERNANCE_MAP_NAMES.iter().all(|name| {
        file_sha256(&root.join(".appsdk/maps").join(name), "governance_map")
            == record_str(
                sdk_map_migration_entry(&manifest, name),
                "/target_digest",
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
    if project.pointer("/sdk/version").and_then(Value::as_str) == Some(target_version)
        && !canonical_source_matches
    {
        let historical_custom_maps = SDK_MAP_MIGRATION_STEPS
            .iter()
            .filter(|prior_step| !sdk_map_migration_checks_live_target(prior_step))
            .any(|prior_step| {
                if !sdk_map_migration_root(root, prior_step)
                    .join("record.json")
                    .is_file()
                {
                    return false;
                }
                let prior_record = assert_sdk_migration_record(root, prior_step, false)
                    .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
                let has_custom_binding = prior_record
                    .get("maps")
                    .and_then(Value::as_array)
                    .is_some_and(|maps| {
                        maps.iter().any(|entry| {
                            entry
                                .get("canonical_source_digest")
                                .is_some_and(Value::is_string)
                                || entry
                                    .get("canonical_target_digest")
                                    .is_some_and(Value::is_string)
                        })
                    });
                has_custom_binding
                    && GOVERNANCE_MAP_NAMES.iter().all(|name| {
                        let entry = prior_record
                            .get("maps")
                            .and_then(Value::as_array)
                            .and_then(|maps| {
                                maps.iter().find(|entry| {
                                    entry.get("name").and_then(Value::as_str) == Some(name)
                                })
                            })
                            .unwrap_or_else(|| fail("INVALID_SDK_MIGRATION_RECORD"));
                        let live =
                            file_sha256(&root.join(".appsdk/maps").join(name), "governance_map");
                        live == record_str(entry, "/source_digest", "sdk-migration-map")
                            || live == record_str(entry, "/target_digest", "sdk-migration-map")
                    })
            });
        let prior_bundle_transition_required = SDK_MAP_MIGRATION_STEPS
            .iter()
            .filter(|prior_step| !sdk_map_migration_checks_live_target(prior_step))
            .any(|prior_step| {
                let record_path = sdk_map_migration_root(root, prior_step).join("record.json");
                if !record_path.is_file() {
                    return false;
                }
                let prior_record: Value = serde_json::from_str(
                    &fs::read_to_string(&record_path)
                        .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD")),
                )
                .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD"));
                prior_record
                    .get("bundle_digest")
                    .and_then(Value::as_str)
                    .is_some_and(|digest| digest != sdk_bundle_digest())
            });
        let prior_witness = SDK_MAP_MIGRATION_STEPS
            .iter()
            .filter(|prior_step| !sdk_map_migration_checks_live_target(prior_step))
            .any(|prior_step| {
                let record_path = sdk_map_migration_root(root, prior_step).join("record.json");
                if !record_path.is_file() {
                    return false;
                }
                let prior_record: Value = serde_json::from_str(
                    &fs::read_to_string(&record_path)
                        .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD")),
                )
                .unwrap_or_else(|_| fail("INVALID_SDK_MIGRATION_RECORD"));
                migration_bundle_transition_digest(root, &prior_record).is_some()
            });
        if current_target_matches {
            if prior_bundle_transition_required && !prior_witness {
                fail("SDK_MIGRATION_BUNDLE_WITNESS_REQUIRED");
            }
            return;
        }
        if historical_custom_maps {
            if prior_bundle_transition_required && !prior_witness {
                fail("SDK_MIGRATION_BUNDLE_WITNESS_REQUIRED");
            }
            return;
        }
        let detail = GOVERNANCE_MAP_NAMES
            .iter()
            .find(|name| {
                file_sha256(&root.join(".appsdk/maps").join(name), "governance_map")
                    != record_str(
                        sdk_map_migration_entry(&manifest, name),
                        "/target_digest",
                        "sdk-map-migration",
                    )
            })
            .copied()
            .unwrap_or("mixed");
        fail(format!("SDK_MIGRATION_LIVE_MAP_UNRECONCILED:{detail}"));
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
                && !sdk_historical_review_map_binding(root, module_id, &review, &review_name)
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
    let staging = migrations.join(format!(".{step}.staging"));
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
            "snapshot_path": format!(".appsdk/migrations/{step}/maps/{}", name)
        }));
    }
    atomic_write_json(
        &staging.join("record.json"),
        &serde_json::json!({
            "schema_version": 1,
            "migration_id": format!("appsdk-{step}"),
            "source_version": source_version,
            "target_version": target_version,
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
    install_governance_maps(root, step, false);
    let _ = assert_sdk_migration_record(root, step, true);
    assert_governance_maps(root);
}

pub(super) fn write_legacy_migration_step(root: &Path, source_version: &str) {
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

pub(super) fn install_current_project_contract(
    root: &Path,
    relative: &str,
    canonical: &str,
    replace_legacy: bool,
) -> bool {
    let target = root.join(relative);
    assert_no_symlink_components(root, &target, "governance_contract_migration");
    let canonical: Value = serde_json::from_str(canonical)
        .unwrap_or_else(|_| fail("INVALID_CANONICAL_RECORD_CONTRACT"));
    if !replace_legacy {
        if target.is_file() {
            let current: Value = serde_json::from_str(
                &fs::read_to_string(&target)
                    .unwrap_or_else(|_| fail("SDK_RECORD_CONTRACT_MIGRATION_READ_FAILED")),
            )
            .unwrap_or_else(|_| fail("SDK_RECORD_CONTRACT_MIGRATION_READ_FAILED"));
            if current == canonical {
                return false;
            }
        }
    }
    let mut content = serde_json::to_vec_pretty(&canonical)
        .unwrap_or_else(|_| fail("SDK_RECORD_CONTRACT_MIGRATION_WRITE_FAILED"));
    content.push(b'\n');
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|_| fail("SDK_RECORD_CONTRACT_MIGRATION_WRITE_FAILED"));
        assert_no_symlink_components(root, parent, "governance_contract_migration");
    }
    atomic_write_bytes(
        &target,
        &content,
        "SDK_RECORD_CONTRACT_MIGRATION_WRITE_FAILED",
    );
    true
}

pub(super) fn install_current_project_contracts(
    root: &Path,
    prefixes: &[&str],
    replace_legacy: bool,
) -> bool {
    let mut changed = false;
    for &(relative, _, canonical) in SDK_BUNDLE_RESOURCES
        .iter()
        .filter(|(path, _, _)| prefixes.iter().any(|prefix| path.starts_with(prefix)))
    {
        changed |= install_current_project_contract(root, relative, canonical, replace_legacy);
    }
    changed
}

pub(super) fn install_current_record_contracts(root: &Path) -> bool {
    install_current_project_contracts(root, &["contracts/records/"], false)
}

const CANONICAL_ZONE_TRANSITION_CONTRACT_PATH: &str =
    "contracts/transitions/zone-transition.manifest.json";
const LEGACY_ZONE_TRANSITION_CONTRACT_PATH: &str =
    "contracts/transitions/zone-transition-manifest.json";

// Official canonical runtime blobs before ed649d9, derived from tagged source:
// v0.1.3 -> a6468f..., v0.1.4 -> 6c485a..., v0.1.5/v0.1.6 -> 456866...
const TRUSTED_LEGACY_ZONE_TRANSITION_CONTRACTS: [&str; 3] = [
    "sha256:a6468f12b64d3e0125ddd77828a4eeeee48cf3a38a0ee6d5bfe56935cd8a1957",
    "sha256:6c485a138ab5a657b760969be42b167ebd034f8a43446609505c2f5d16d5afab",
    "sha256:4568668437b4e0b44db4709d27e31c2783c8a6e4ccd828273a4675775f69ca1f",
];

pub(super) struct TransitionContractRefreshPlan {
    targets: Vec<TransitionContractRefreshTarget>,
}

struct TransitionContractRefreshTarget {
    relative: &'static str,
    write: bool,
}

pub(super) fn preflight_current_transition_contracts(
    root: &Path,
    project: &Value,
) -> TransitionContractRefreshPlan {
    let declared = project
        .pointer("/governance/zone_transition_contract")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            fail("INVALID_GOVERNANCE_CONTRACT:/governance/zone_transition_contract")
        });
    let relatives = match declared {
        CANONICAL_ZONE_TRANSITION_CONTRACT_PATH => {
            vec![CANONICAL_ZONE_TRANSITION_CONTRACT_PATH]
        }
        LEGACY_ZONE_TRANSITION_CONTRACT_PATH => vec![
            CANONICAL_ZONE_TRANSITION_CONTRACT_PATH,
            LEGACY_ZONE_TRANSITION_CONTRACT_PATH,
        ],
        _ => fail(format!("UNSUPPORTED_ZONE_TRANSITION_CONTRACT:{declared}")),
    };
    let mut targets = Vec::with_capacity(relatives.len());
    for relative in relatives {
        let target = safe_owned_path(root, relative, "zone_transition_contract");
        let write = match fs::symlink_metadata(&target) {
            Ok(metadata) if !metadata.is_file() => {
                fail(format!("GOVERNANCE_CONTRACT_NOT_FILE:{relative}"));
            }
            Ok(_) => {
                let bytes = fs::read(&target).unwrap_or_else(|_| {
                    fail(format!("SDK_TRANSITION_CONTRACT_READ_FAILED:{relative}"))
                });
                if bytes == CANONICAL_ZONE_TRANSITION_CONTRACT.as_bytes() {
                    false
                } else if trusted_legacy_zone_transition_contract(&bytes) {
                    true
                } else {
                    fail(format!(
                        "SDK_TRANSITION_CONTRACT_UNKNOWN_CONTENT:{relative}"
                    ));
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => true,
            Err(_) => fail(format!(
                "SDK_TRANSITION_CONTRACT_METADATA_FAILED:{relative}"
            )),
        };
        targets.push(TransitionContractRefreshTarget { relative, write });
    }
    TransitionContractRefreshPlan { targets }
}

pub(super) fn install_current_transition_contracts(
    root: &Path,
    plan: &TransitionContractRefreshPlan,
) -> bool {
    let mut changed = false;
    for target in &plan.targets {
        if !target.write {
            continue;
        }
        let path = root.join(target.relative);
        assert_no_symlink_components(root, &path, "zone_transition_contract");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|_| fail("SDK_TRANSITION_CONTRACT_WRITE_FAILED"));
            assert_no_symlink_components(root, parent, "zone_transition_contract");
        }
        atomic_write_bytes(
            &path,
            CANONICAL_ZONE_TRANSITION_CONTRACT.as_bytes(),
            "SDK_TRANSITION_CONTRACT_WRITE_FAILED",
        );
        changed = true;
    }
    changed
}

fn trusted_legacy_zone_transition_contract(bytes: &[u8]) -> bool {
    let digest = digest_bytes(bytes);
    TRUSTED_LEGACY_ZONE_TRANSITION_CONTRACTS
        .iter()
        .any(|known| *known == digest.as_str())
}

pub(super) fn assert_fresh_project_contract_target(root: &Path, relative: &str) {
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

pub(super) fn assert_fresh_project_contract_targets(root: &Path) {
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
