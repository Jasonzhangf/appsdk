use super::*;

pub(super) fn assert_development_scenarios(root: &Path, project: &Value) -> DevelopmentScenarios {
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
        "../../../contracts/development-scenarios.manifest.json"
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
    DevelopmentScenarios {
        multi_worker_collaboration: multi_worker,
        multi_worktree_merge_queue: multi_worktree,
    }
}

pub(super) fn assert_project_contract(root: &Path, project: &Value) {
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
    if project
        .pointer("/sdk/bundle_manifest")
        .and_then(Value::as_str)
        != Some(".appsdk/contracts/sdk-bundle.manifest.json")
    {
        fail("INVALID_SDK_CONTRACT:/sdk/bundle_manifest");
    }
    if project
        .pointer("/sdk/resource_record")
        .and_then(Value::as_str)
        != Some(".appsdk/sdk-resources.json")
    {
        fail("INVALID_SDK_CONTRACT:/sdk/resource_record");
    }
    let sdk_version = project
        .pointer("/sdk/version")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/sdk/version"));
    if sdk_version != SDK_VERSION {
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
        let _ = assert_registry_binding_contract(module, id);
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
        let stage = module
            .get("stage")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail(format!("INVALID_PROJECT_MODULE:{}", id)));
        assert_module_regression_contract(module, stage, id);
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

pub(super) fn compile(root: &Path) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    let project = read_project(root);
    assert_compile_preconditions(root, &project, None);
    assert_declared_contracts(root, &project);
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    // Publish the deterministic project projection before per-module builds.
    // A later module failure must not leave verification bound to an older
    // lifecycle snapshot; the projection itself contains no build output.
    write_artifact(root, &project);
    let control_snapshot = compile_control_snapshot(root, &project);
    for module in modules {
        let module_id = record_str(module, "/module_id", "module");
        if module.get("stage").and_then(Value::as_str) != Some("frozen") {
            // Module builds are external commands. Recheck the mutable control
            // inputs at each boundary while reusing the already validated
            // project value and avoiding a second full contract parse.
            assert_compile_control_snapshot(root, &project, &control_snapshot);
            compile_module_with_project(root, &project, module_id);
        }
    }
    assert_compile_control_snapshot(root, &project, &control_snapshot);
    let artifact = write_artifact(root, &project);
    println!("{}", serde_json::to_string_pretty(&artifact).unwrap());
}

pub(super) fn write_project(root: &Path, project: &Value) {
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

pub(super) fn begin_version(root: &Path, module_id: &str, from: &str, to: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    assert_version(from, "INVALID_ACTIVE_VERSION");
    assert_version(to, "INVALID_ACTIVE_VERSION");
    if from == to {
        fail("MODULE_VERSION_MUST_ADVANCE");
    }
    let project = read_project(root);
    assert_declared_contracts(root, &project);
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

pub(super) fn stage_protected_archive(
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

pub(super) fn assert_protected_archive_matches(
    root: &Path,
    module: &Value,
    artifact: &Value,
    archive: &Path,
) {
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

pub(super) fn restore_active_from_archive(
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

pub(super) fn rehydrate_transaction_dir(root: &Path, module_id: &str) -> PathBuf {
    root.join(".appsdk")
        .join("transactions")
        .join(format!("rehydrate-{}", module_id))
}

pub(super) fn read_rehydrate_transaction(
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

pub(super) fn write_rehydrate_transaction(
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

pub(super) fn active_version_projection_matches(
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

pub(super) fn assert_previous_active_projection_matches(
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

pub(super) fn assert_active_projection_matches(
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
