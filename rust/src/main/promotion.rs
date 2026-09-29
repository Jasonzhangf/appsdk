use super::*;

pub(super) fn promote(root: &Path, target: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    let project = read_project(root);
    assert_project_contract(root, &project);
    assert_goal_confirmed(root);
    assert_declared_contracts(root, &project);
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

pub(super) fn promote_module(root: &Path, module_id: &str, target: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    if target == "frozen" && recover_freeze_transaction(root, &project, module_id) {
        println!("{}", serde_json::to_string_pretty(&project).unwrap());
        return;
    }
    assert_declared_contracts(root, &project);
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

pub(super) fn freeze_module(root: &Path, module_id: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    if recover_freeze_transaction(root, &project, module_id) {
        println!("{}", serde_json::to_string_pretty(&project).unwrap());
        return;
    }
    assert_declared_contracts(root, &project);
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
    assert_module_regression_contract(&candidate["modules"][index], "frozen", module_id);
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

pub(super) fn publish_active(root: &Path, module_id: &str, version: &str) {
    publish_active_internal(root, module_id, version, false);
}

pub(super) fn publish_active_rehydrated(root: &Path, module_id: &str, version: &str) {
    publish_active_internal(root, module_id, version, true);
}

pub(super) fn publish_active_internal(
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
    assert_declared_contracts(root, &project);
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

pub(super) fn assert_sdk_resources(root: &Path, required: bool, allow_reset_resource_gaps: bool) {
    let path = root.join(".appsdk/sdk-resources.json");
    if !path.exists() {
        if required && !allow_reset_resource_gaps {
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
        || record.get("version").and_then(Value::as_str) != Some(SDK_VERSION)
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
    if entries.is_empty() {
        fail("INVALID_SDK_RESOURCES");
    }
    let bundle_entries = sdk_bundle_resource_entries();
    let mut known = BTreeSet::new();
    for (source, class, _) in &bundle_entries {
        if !known.insert(format!("{}\0{}", class, source)) {
            fail("SDK_RESOURCE_BUNDLE_DUPLICATE");
        }
    }
    let mut seen = BTreeSet::new();
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
        let class = entry
            .get("class")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fail(format!("INVALID_SDK_RESOURCE_CLASS:{}", source)));
        let key = format!("{}\0{}", class, source);
        if !known.contains(&key) {
            fail(format!("SDK_RESOURCE_UNKNOWN:{}", source));
        }
        if !seen.insert(key) {
            fail(format!("SDK_RESOURCE_DUPLICATE:{}", source));
        }
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
        let expected_relative = sdk_resource_install_relative(source, class);
        if relative != expected_relative {
            fail(format!("SDK_RESOURCE_PATH_MISMATCH:{}", relative));
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
        let target = root.join(relative);
        assert_no_symlink_components(root, &target, "sdk_resource_record");
        if !target.exists() {
            if allow_reset_resource_gaps {
                eprintln!(
                    "warning: SDK resource missing after authorized governance reset ({})",
                    relative
                );
                continue;
            }
            fail(format!("SDK_RESOURCE_MISMATCH:{}", relative));
        }
        if !target.is_file() || file_sha256(&target, "sdk_resource") != expected {
            fail(format!("SDK_RESOURCE_MISMATCH:{}", relative));
        }
        let project_record_contract = class == "contracts"
            && source.starts_with("contracts/records/")
            && CANONICAL_RECORD_CONTRACTS.contains(&source);
        let source_path = root.join(source);
        if project_record_contract && source_path.is_file() {
            assert_no_symlink_components(root, &source_path, "sdk_resource_source");
            if file_sha256(&source_path, "sdk_resource_source") != expected {
                fail(format!("SDK_RESOURCE_SOURCE_MISMATCH:{}", source));
            }
        }
    }
    for (source, class, _) in bundle_entries {
        let key = format!("{}\0{}", class, source);
        if !seen.contains(&key) {
            if allow_reset_resource_gaps {
                eprintln!(
                    "warning: SDK resource record entry missing after authorized governance reset ({})",
                    source
                );
                continue;
            }
            fail(format!("SDK_RESOURCE_RECORD_ENTRY_MISSING:{}", source));
        }
    }
}

pub(super) fn reset_governance_record_mode(root: &Path) -> Option<String> {
    let path = root
        .join(".appsdk")
        .join("records")
        .join("reset-governance-record.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(_) => fail("INVALID_RESET_GOVERNANCE_RECORD"),
    };
    if metadata.file_type().is_symlink() {
        fail("GOVERNANCE_PATH_SYMLINK:reset_governance_record");
    }
    if !metadata.is_file() {
        fail("INVALID_RESET_GOVERNANCE_RECORD");
    }
    let record: Value = serde_json::from_str(
        &fs::read_to_string(&path).unwrap_or_else(|_| fail("INVALID_RESET_GOVERNANCE_RECORD")),
    )
    .unwrap_or_else(|_| fail("INVALID_RESET_GOVERNANCE_RECORD"));
    if record.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail("INVALID_RESET_GOVERNANCE_RECORD");
    }
    let mode = record
        .get("mode")
        .and_then(Value::as_str)
        .filter(|mode| matches!(*mode, "fresh_init" | "discard_legacy_control_plane"))
        .unwrap_or_else(|| fail("INVALID_RESET_GOVERNANCE_RECORD"));
    for key in ["reset_id", "branch"] {
        if record
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            fail("INVALID_RESET_GOVERNANCE_RECORD");
        }
    }
    let transaction_id = record
        .get("transaction_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    if record.get("transaction_id").is_some() && transaction_id.is_none() {
        fail("INVALID_RESET_GOVERNANCE_RECORD");
    }
    match transaction_id {
        Some(_) if record.get("reset_id") != record.get("transaction_id") => {
            fail("INVALID_RESET_GOVERNANCE_RECORD");
        }
        Some(_) => {}
        None if mode == "fresh_init" => fail("INVALID_RESET_GOVERNANCE_RECORD"),
        // Legacy discard receipts predate the transactional reset owner. They
        // bind the operation through the historical `reset-<pid>` identity and
        // are accepted only when the full receipt below validates and the
        // receipt is a committed blob, not a working-tree fabrication.
        None => {
            let reset_id = record
                .get("reset_id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| fail("INVALID_RESET_GOVERNANCE_RECORD"));
            let suffix = reset_id
                .strip_prefix("reset-")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| fail("INVALID_RESET_GOVERNANCE_RECORD"));
            if !suffix.chars().all(|value| value.is_ascii_digit()) {
                fail("INVALID_RESET_GOVERNANCE_RECORD");
            }
            let prefix = Command::new("git")
                .args([
                    "-C",
                    root.to_str().unwrap_or(""),
                    "rev-parse",
                    "--show-prefix",
                ])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                .unwrap_or_default();
            let committed = Command::new("git")
                .args([
                    "-C",
                    root.to_str().unwrap_or(""),
                    "show",
                    &format!("HEAD:{prefix}.appsdk/records/reset-governance-record.json"),
                ])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| output.stdout)
                .is_some_and(|bytes| fs::read(&path).is_ok_and(|current| current == bytes));
            if !committed {
                fail("INVALID_RESET_GOVERNANCE_RECORD");
            }
            eprintln!(
                "warning: authorized legacy governance reset receipt has no transaction_id ({})",
                path.display()
            );
        }
    }
    let created_at = record
        .get("created_at")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail("INVALID_RESET_GOVERNANCE_RECORD"));
    let created_at = DateTime::parse_from_rfc3339(created_at)
        .unwrap_or_else(|_| fail("INVALID_RESET_GOVERNANCE_RECORD"))
        .with_timezone(&Utc);
    if created_at > Utc::now() {
        fail("INVALID_RESET_GOVERNANCE_RECORD");
    }
    for key in ["preserved", "removed"] {
        let values = record
            .get(key)
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty())
            .unwrap_or_else(|| fail("INVALID_RESET_GOVERNANCE_RECORD"));
        if values
            .iter()
            .any(|entry| entry.as_str().filter(|entry| !entry.is_empty()).is_none())
        {
            fail("INVALID_RESET_GOVERNANCE_RECORD");
        }
    }
    Some(mode.to_string())
}

pub(super) fn verify_sdk_migration_record(root: &Path, admission: bool) {
    if !admission && reset_governance_record_mode(root).is_some() {
        for step in SDK_MAP_MIGRATION_STEPS {
            let migration_root = sdk_map_migration_root(root, step);
            if fs::symlink_metadata(&migration_root).is_ok() {
                eprintln!(
                    "warning: SDK migration history ignored after authorized governance reset ({})",
                    migration_root.display()
                );
            }
        }
        return;
    }
    if !admission {
        // A project that still points at an older SDK bundle may continue
        // ordinary development while its immutable migration witness awaits
        // the explicit pin-lock/fresh-init owner.  Keep path integrity checks
        // active, but do not turn an historical bundle mismatch into a
        // development blocker.  Once the lock points at this Bundle, the
        // strict migration validator below remains authoritative.
        for step in SDK_MAP_MIGRATION_STEPS {
            let migration_root = sdk_map_migration_root(root, step);
            if fs::symlink_metadata(&migration_root).is_ok() {
                assert_no_symlink_components(root, &migration_root, "sdk_migration");
                let lock_path = root.join(".appsdk/sdk.lock");
                let lock_bundle = fs::read_to_string(&lock_path)
                    .ok()
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    .and_then(|lock| {
                        lock.get("bundle_digest")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    });
                let current_bundle = sdk_bundle_digest();
                if lock_bundle.as_deref() != Some(current_bundle.as_str()) {
                    eprintln!(
                        "warning: legacy SDK migration history retained for ordinary development ({})",
                        migration_root.display()
                    );
                    return;
                }
            }
        }
    }
    for step in SDK_MAP_MIGRATION_STEPS {
        if sdk_map_migration_root(root, step)
            .join("record.json")
            .is_file()
        {
            let _ =
                assert_sdk_migration_record(root, step, sdk_map_migration_checks_live_target(step));
        }
    }
}

pub(super) fn verify_internal(
    root: &Path,
    admission: bool,
    require_project_artifact: bool,
    check_module_publications: bool,
    emit_result: bool,
) {
    assert_project_root_safe(root);
    let reset_epoch = reset_governance_record_mode(root).is_some();
    let mut delivery_verified = false;
    let mut baseline_status = if reset_epoch { "required" } else { "current" };
    let project = read_project(root);
    assert_governance_maps(root);
    verify_sdk_migration_record(root, admission);
    assert_declared_contracts(root, &project);
    assert_project_contract(root, &project);
    let test_governance = if emit_result {
        match test_governance_report(root, &project, None, admission) {
            Ok(report) => {
                if admission && report.get("status").and_then(Value::as_str) == Some("blocked") {
                    fail("TEST_GOVERNANCE_BLOCKED");
                }
                report
            }
            Err(error) => {
                if admission {
                    fail(error);
                }
                serde_json::json!({
                    "mode": "selected",
                    "status": "blocked",
                    "error": error
                })
            }
        }
    } else {
        Value::Null
    };
    if project.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail("UNSUPPORTED_PROJECT_SCHEMA");
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
        reset_epoch && !admission,
    );
    assert_goal_contract_if_present(root);
    let artifact_file = generated_root(root, &project).join("project.compiled.json");
    let stage = required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT");
    if require_project_artifact
        && matches!(
            stage,
            "compiled" | "controlled_verified" | "architecture_stable" | "frozen" | "retired"
        )
        && !artifact_file.exists()
        && !(reset_epoch && !admission)
    {
        fail("COMPILED_STAGE_REQUIRES_ARTIFACT");
    }
    let artifact = if artifact_file.exists() {
        Some(read_compiled_artifact(root, &project))
    } else {
        None
    };
    if let Some(artifact) = artifact.as_ref() {
        assert_artifact_matches(&project, artifact);
    }
    let development_gap = reset_epoch && !admission;
    let verify_publication_graph = check_module_publications && !development_gap;
    for module in project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"))
    {
        if verify_publication_graph && module.get("stage").and_then(Value::as_str) == Some("frozen")
        {
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
        if development_gap && module.get("stage").and_then(Value::as_str) == Some("frozen") {
            delivery_verified = false;
            baseline_status = "required";
        }
    }
    if verify_publication_graph
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
        let command_ok = true;
        // `verify --admission` runs the delivery checks above; reaching this
        // point means they passed. Ordinary `verify` is a development probe:
        // it may succeed without evaluating delivery, so it must not report
        // delivery as verified or emit a success-shaped `ok`.
        let delivery_assessed = admission;
        if delivery_assessed {
            delivery_verified = true;
        }
        let ok = delivery_verified;
        let final_baseline_status = if reset_epoch && !admission {
            "required"
        } else {
            baseline_status
        };
        let result = serde_json::json!({
            "ok": ok,
            "command_ok": command_ok,
            "project_id": required_str(&project, "/project_id", "INVALID_PROJECT_ID"),
            "stage": required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT"),
            "development_ready": true,
            "delivery_verified": delivery_verified,
            "delivery_assessed": delivery_assessed,
            "test_governance": test_governance,
            "baseline_status": final_baseline_status,
            "reason": if reset_epoch && !admission {
                Value::String("baseline_required".into())
            } else if delivery_assessed {
                Value::Null
            } else {
                Value::String("delivery_not_evaluated".into())
            }
        });
        println!("{}", result);
    }
}

pub(super) fn verify(root: &Path, admission: bool) {
    verify_internal(root, admission, true, true, true);
}
