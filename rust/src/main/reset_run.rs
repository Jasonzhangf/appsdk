use super::*;

pub(super) fn reset_transaction_build_staging(
    root: &Path,
    transaction_dir: &Path,
    generated_roots: &[String],
    transaction_id: &str,
    branch: &str,
    mode: ResetMode,
) -> Result<(), String> {
    let staging_root = transaction_dir.join("staging");
    reset_transaction_symlink_components(transaction_dir, &staging_root)?;
    fs::create_dir_all(&staging_root)
        .map_err(|error| format!("GOVERNANCE_RESET_STAGING_CREATE_FAILED:{error}"))?;
    let binary = env::current_exe()
        .map_err(|error| format!("GOVERNANCE_RESET_STAGING_BINARY_FAILED:{error}"))?;
    let output = Command::new(binary)
        .args([
            "reset-staging-scaffold",
            root.to_str().unwrap_or(""),
            transaction_dir.to_str().unwrap_or(""),
            transaction_id,
        ])
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
    // Start from the stable scaffold, then replace every root contract declared
    // by the current bundle, including record contracts added after the
    // scaffold template was published.  The staging helper preserves the
    // existing project contract before this replacement is published.
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
    // Requirements are project-owned truth, not rebuildable governance state.
    // The caller holds the same transaction lock used by requirements apply.
    if crate::requirements::read_requirements_if_present(root).is_some() {
        let bytes = fs::read(root.join(".appsdk/requirements.json"))
            .map_err(|error| format!("REQUIREMENTS_PRESERVE_READ_FAILED:{error}"))?;
        reset_transaction_write_bytes(
            transaction_dir,
            &staging_root.join(".appsdk/requirements.json"),
            &bytes,
        )?;
    }
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
        "mode": mode.record_mode(),
        "preserved": ["business_source", "runtime_data", "active", "protected", "user_requirements"],
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

pub(super) fn reset_transaction_rollback(
    root: &Path,
    transaction_dir: &Path,
    transaction_id: &str,
    targets: &[ResetTransactionTarget],
    created_dirs: &[String],
    generated_roots: &[String],
    mode: ResetMode,
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
            mode,
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
            mode,
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

pub(super) fn reset_transaction_rollback_or_combine(
    root: &Path,
    transaction_dir: &Path,
    transaction_id: &str,
    targets: &[ResetTransactionTarget],
    created_dirs: &[String],
    generated_roots: &[String],
    mode: ResetMode,
    cause: &str,
) -> String {
    match reset_transaction_rollback(
        root,
        transaction_dir,
        transaction_id,
        targets,
        created_dirs,
        generated_roots,
        mode,
        cause,
    ) {
        Ok(()) => cause.to_string(),
        Err(error) => format!("{cause};{error}"),
    }
}

pub(super) fn reset_transaction_run(
    root: &Path,
    branch: &str,
    generated_roots: &[String],
    mode: ResetMode,
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
    let transaction_id = reset_transaction_id(mode)?;
    let empty_targets = Vec::new();
    let empty_created = Vec::new();
    reset_transaction_marker(
        &transaction_dir,
        &transaction_id,
        root,
        mode,
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
        mode,
    ) {
        let _ = reset_transaction_marker(
            &transaction_dir,
            &transaction_id,
            root,
            mode,
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
                mode,
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
        mode,
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
                mode,
                &cause,
            );
            return Err(failure);
        }
        targets[index].quarantined = true;
        if let Err(error) = reset_transaction_marker(
            &transaction_dir,
            &transaction_id,
            root,
            mode,
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
                mode,
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
                mode,
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
                    mode,
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
                mode,
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
                mode,
                &cause,
            );
            return Err(failure);
        }
        targets[index].published = true;
        if let Err(error) = reset_transaction_marker(
            &transaction_dir,
            &transaction_id,
            root,
            mode,
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
                mode,
                &error,
            );
            return Err(failure);
        }
    }
    reset_transaction_marker(
        &transaction_dir,
        &transaction_id,
        root,
        mode,
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
                mode,
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
            mode,
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
