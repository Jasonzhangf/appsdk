use super::*;

pub(super) fn finish_rehydrate_transaction(root: &Path, module_id: &str) {
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

pub(super) fn protected_archive_needs_version_restore(root: &Path, archive: &Path, artifact: &Value) -> bool {
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

pub(super) fn restore_current_protected_archive_from_version(
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

pub(super) fn restore_generated_module_from_archive(
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

pub(super) fn verify_rehydrated_module(
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

pub(super) fn rehydrate_frozen(root: &Path, module_id: &str) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    assert_project_contract(root, &project);
    assert_declared_contracts(root, &project);
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

pub(super) fn freeze_transaction_dir(root: &Path, module_id: &str) -> PathBuf {
    root.join(".appsdk")
        .join("transactions")
        .join(format!("freeze-{}", module_id))
}

pub(super) fn recover_freeze_transaction(root: &Path, project: &Value, module_id: &str) -> bool {
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
