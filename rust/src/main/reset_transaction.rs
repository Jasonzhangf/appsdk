use super::*;

pub(super) fn print_reset_result(mode: ResetMode) {
    println!(
        "{}",
        serde_json::json!({
            "operation": "governance.reset",
            "mode": mode.record_mode(),
            "status": "completed",
            "development_ready": true,
            "delivery_verified": false,
            "baseline_status": "required",
            "registration_status": "pending",
            "next_action": "run_applicable_validation"
        })
    );
}

#[cfg(unix)]
pub(super) fn reset_transaction_symlink_components(base: &Path, path: &Path) -> Result<(), String> {
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

pub(super) fn reset_transaction_lock_path(root: &Path) -> PathBuf {
    let transaction_dir = reset_transaction_dir(root);
    PathBuf::from(format!("{}.lock", transaction_dir.display()))
}

pub(super) fn reset_transaction_acquire_lock(root: &Path) -> Result<fs::File, String> {
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
    // Keep the pathname stable for the lifetime of the project. `flock`
    // releases the kernel lock when this descriptor closes, including after a
    // crash; unlinking here would let a contender that opened the old inode
    // race a new contender on a replacement inode.
    Ok(file)
}

pub(super) fn reset_transaction_expected_target_kind(
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

pub(super) fn reset_transaction_allowed_target_relative(relative: &str, generated_roots: &[String]) -> bool {
    reset_transaction_expected_target_kind(relative, generated_roots).is_some()
}

pub(super) fn reset_transaction_contract_target_relative(relative: &str) -> bool {
    relative.starts_with("contracts/records/") || relative.starts_with("contracts/transitions/")
}

pub(super) fn reset_transaction_expected_staged_relative(relative: &str) -> Option<String> {
    if relative == ".appsdk" || relative == ".appsdk-control" || relative == ".gitignore" {
        Some(format!("staging/{relative}"))
    } else if reset_transaction_contract_target_relative(relative) || relative == "generated" {
        Some(format!("staging/{relative}"))
    } else {
        None
    }
}

pub(super) fn reset_transaction_allowed_created_dir_relative(relative: &str) -> bool {
    matches!(
        relative,
        "contracts" | "contracts/records" | "contracts/transitions"
    )
}

pub(super) fn reset_transaction_parse_generated_root(
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

pub(super) fn reset_transaction_parse_generated_roots(
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

pub(super) fn reset_transaction_generated_roots_with_current_baseline(
    project: &Value,
    case_insensitive: bool,
) -> Result<Vec<String>, String> {
    if project.pointer("/governance/generated_root").is_none() {
        return Ok(vec!["generated".to_string()]);
    }
    reset_transaction_parse_generated_roots(project, case_insensitive)
}

pub(super) fn reset_transaction_generated_root_allowed(relative: &str) -> bool {
    reset_transaction_validate_relative(relative).is_ok()
        && reset_transaction_parse_generated_root(relative, true).is_ok()
}

pub(super) fn reset_transaction_created_dirs(
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

pub(super) fn reset_transaction_read_project_contract(path: &Path) -> Result<Value, String> {
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

pub(super) fn reset_transaction_recovery_project_contract(
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

pub(super) fn reset_transaction_recovery_generated_roots(
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
        reset_transaction_generated_roots_with_current_baseline(&project, case_insensitive)
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

pub(super) fn reset_transaction_marker_generated_roots(
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

pub(super) fn reset_transaction_committed_record_matches(root: &Path, marker: &Value) -> bool {
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
    // Both shared-engine modes publish identical `reset_id`/`transaction_id`
    // receipts. New markers bind the mode as well; legacy markers without a
    // mode remain readable only for the historical fresh-init path.
    let record_mode = record.get("mode").and_then(Value::as_str);
    let mode_matches = match marker.get("mode").and_then(Value::as_str) {
        Some(mode) => record_mode == Some(mode),
        None => record_mode == Some("fresh_init"),
    };
    mode_matches
        && record.get("transaction_id").and_then(Value::as_str) == Some(transaction_id)
        && record.get("reset_id").and_then(Value::as_str) == Some(transaction_id)
}

pub(super) fn reset_transaction_marker_root_matches(
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

pub(super) fn reset_transaction_validate_marker(
    root: &Path,
    transaction_dir: &Path,
    marker: &Value,
) -> Result<(), String> {
    if marker.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid schema_version".into());
    }
    if let Some(mode) = marker.get("mode") {
        if !matches!(
            mode.as_str(),
            Some("fresh_init" | "discard_legacy_control_plane")
        ) {
            return Err("GOVERNANCE_RESET_RECOVERY_REQUIRED:invalid mode".into());
        }
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
