use super::*;

pub(super) fn reset_transaction_dir(root: &Path) -> PathBuf {
    let name = root
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("project");
    root.parent()
        .unwrap_or(root)
        .join(format!(".appsdk-reset-transaction-{name}"))
}

pub(super) fn reset_transaction_id(mode: ResetMode) -> Result<String, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("RESET_TRANSACTION_NONCE_FAILED:{error}"))?
        .as_nanos();
    let prefix = match mode {
        ResetMode::FreshInit => "fresh-init",
        ResetMode::DiscardLegacy => "reset-governance",
    };
    Ok(format!("{prefix}-{}-{nonce}", std::process::id()))
}

pub(super) fn reset_transaction_write_bytes(
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

pub(super) fn reset_transaction_is_marker_temp(name: &str) -> bool {
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

pub(super) fn reset_transaction_write_json(
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

pub(super) fn reset_transaction_remove_path(base: &Path, path: &Path) -> Result<(), String> {
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

pub(super) fn reset_transaction_directory_empty(path: &Path) -> Result<bool, String> {
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

pub(super) fn reset_transaction_validate_quarantine_entries(
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

pub(super) fn reset_transaction_validate_cleanup_layout(
    transaction_dir: &Path,
) -> Result<(), String> {
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

pub(super) fn reset_transaction_remove_marker_temps(transaction_dir: &Path) -> Result<(), String> {
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

pub(super) fn reset_transaction_cleanup_committed(transaction_dir: &Path) -> Result<(), String> {
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

pub(super) fn reset_transaction_recover_unmarked(
    transaction_dir: &Path,
) -> Result<Option<bool>, String> {
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

pub(super) fn reset_transaction_target_value(
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

pub(super) fn reset_transaction_marker(
    transaction_dir: &Path,
    transaction_id: &str,
    root: &Path,
    mode: ResetMode,
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
        "mode": mode.record_mode(),
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

pub(super) fn reset_transaction_read_marker(transaction_dir: &Path) -> Result<Value, String> {
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

pub(super) fn reset_transaction_validate_relative(relative: &str) -> Result<(), String> {
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

pub(super) fn reset_transaction_recover(root: &Path) -> Result<Option<bool>, String> {
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

pub(super) fn reset_transaction_fresh_project_targets() -> Vec<String> {
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

pub(super) fn reset_transaction_expected_target_relatives(
    generated_roots: &[String],
) -> BTreeSet<String> {
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

pub(super) fn reset_transaction_add_target(
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

pub(super) fn reset_transaction_build_targets(
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

pub(super) fn reset_transaction_quarantine_generated_roots(
    generated_roots: &[String],
) -> Vec<String> {
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
