use super::*;

pub(super) fn atomic_write_bytes(target: &Path, bytes: &[u8], error: &str) {
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

/// Materialize the ignored `.appsdk/sdk.bin` witness from the exact pinned
/// AppSDK executable. The witness must stay byte-identical to the lock digest
/// and keep the executable mode a consumer gate expects, so callers resolving
/// a missing witness in a fresh checkout do not have to hand-copy a binary.
pub(super) fn write_sdk_witness(root: &Path, source: &Path) {
    let witness = root.join(".appsdk/sdk.bin");
    if fs::symlink_metadata(&witness)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_binary");
    }
    let bytes = fs::read(source).unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
    atomic_write_bytes(&witness, &bytes, "SDK_BINARY_WRITE_FAILED");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(source)
            .map(|metadata| metadata.permissions().mode() & 0o777)
            .unwrap_or(0o755);
        let mode = if mode & 0o111 == 0 {
            mode | 0o755
        } else {
            mode
        };
        let _ = fs::set_permissions(&witness, fs::Permissions::from_mode(mode));
    }
}

/// Recreate the ignored `.appsdk/sdk.bin` witness in a clean checkout from the
/// exact pinned AppSDK executable.
///
/// A Git worktree inherits the tracked `.appsdk/sdk.lock` but never the ignored
/// witness, so a legacy lock that still carries `binary_ref: "project-sdk"`
/// cannot satisfy a consumer gate that inspects the witness. Only a project
/// whose lock still pins the historical `project-sdk` reference is affected;
/// current locks omit the pin and resolve nothing. The witness must stay
/// byte-identical to the locked digest, so a genuinely missing or mismatched
/// pinned binary fails closed instead of writing an unverifiable witness.
pub(super) fn resolve_sdk_witness(root: &Path, source: &Path) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    let lock_path = root.join(".appsdk/sdk.lock");
    if fs::symlink_metadata(&lock_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_lock");
    }
    let lock: Value = serde_json::from_str(
        &fs::read_to_string(&lock_path).unwrap_or_else(|_| fail("MISSING_SDK_LOCK")),
    )
    .unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
    if lock.get("binary_ref").and_then(Value::as_str) != Some("project-sdk") {
        println!("no project-sdk witness required for this lock");
        return;
    }
    let expected = lock
        .get("digest")
        .and_then(Value::as_str)
        .filter(|digest| {
            digest.len() == 71
                && digest.starts_with("sha256:")
                && digest[7..].chars().all(|c| c.is_ascii_hexdigit())
        })
        .unwrap_or_else(|| fail("INVALID_SDK_LOCK_DIGEST"));
    assert_no_symlink_components(root, &root.join(".appsdk"), "appsdk_control");
    let bytes = fs::read(source).unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
    let actual = digest_bytes(&bytes);
    if actual != expected {
        fail(format!(
            "SDK_WITNESS_BINARY_MISMATCH:{expected}:{actual}; supply the pinned AppSDK binary with --binary"
        ));
    }
    write_sdk_witness(root, source);
    println!(
        "resolved {} from {}",
        root.join(".appsdk/sdk.bin").display(),
        source.display()
    );
}

pub(super) fn atomic_write_json(target: &Path, value: &Value, error: &str) {
    atomic_write_bytes(
        target,
        (serde_json::to_string_pretty(value).unwrap() + "\n").as_bytes(),
        error,
    );
}

pub(super) fn read_record(root: &Path, name: &str) -> Value {
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

pub(super) fn write_record(root: &Path, name: &str, record: &Value) {
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

pub(super) fn producer_input_path(root: &Path, raw: &str) -> PathBuf {
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

pub(super) fn producer_string(record: &Value, path: &str, error: &str) -> String {
    record
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fail(error))
}

pub(super) fn producer_issue(record: &Value, path: &str, error: &str) -> String {
    record
        .pointer(path)
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| fail(error))
}

pub(super) fn producer_goal_issue_binding(goal_issue_id: &str, issue_id: &str) -> String {
    sha256(&canonical(&serde_json::json!({
        "goal_issue_id": goal_issue_id,
        "issue_id": issue_id
    })))
}

pub(super) fn producer_goal_issue_binding_for_input(
    goal: &Value,
    worktree: &Value,
    worktree_issue: &str,
) -> Option<(String, String)> {
    let goal_issue_id = match goal.get("issue_id") {
        Some(Value::String(value)) => Some(value.as_str()),
        Some(Value::Null) | None => None,
        Some(_) => fail("PRODUCER_GOAL_ISSUE_MISMATCH"),
    };
    let declared_goal_issue_id = worktree
        .get("goal_issue_id")
        .map(|_| producer_string(worktree, "/goal_issue_id", "PRODUCER_GOAL_ISSUE_MISMATCH"));
    match (goal_issue_id, declared_goal_issue_id) {
        (Some(goal_issue_id), Some(declared_goal_issue_id))
            if declared_goal_issue_id == goal_issue_id =>
        {
            Some((
                declared_goal_issue_id,
                producer_goal_issue_binding(goal_issue_id, worktree_issue),
            ))
        }
        (Some(goal_issue_id), None) if goal_issue_id == worktree_issue => None,
        (None, None)
            if worktree_issue.is_empty()
                || worktree_issue == "none"
                || worktree_issue.starts_with("legacy-") =>
        {
            None
        }
        _ => fail("PRODUCER_GOAL_ISSUE_MISMATCH"),
    }
}

pub(super) fn assert_producer_goal_issue_binding(worktree: &Value, issue_id: &str) {
    match worktree.get("goal_issue_id") {
        Some(goal_issue_id) => {
            let goal_issue_id = goal_issue_id
                .as_str()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| fail("PRODUCER_GOAL_ISSUE_BINDING_MISMATCH"));
            let expected = producer_goal_issue_binding(goal_issue_id, issue_id);
            if worktree.get("goal_issue_binding").and_then(Value::as_str) != Some(expected.as_str())
            {
                fail("PRODUCER_GOAL_ISSUE_BINDING_MISMATCH");
            }
        }
        None if worktree.get("goal_issue_binding").is_some() => {
            fail("PRODUCER_GOAL_ISSUE_BINDING_MISMATCH")
        }
        None => {}
    }
}

pub(super) fn producer_bool(record: &Value, path: &str, error: &str) {
    if record.pointer(path) != Some(&Value::Bool(true)) {
        fail(error);
    }
}

pub(super) fn producer_command_dir(root: &Path, raw: &str) -> Result<PathBuf, &'static str> {
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

pub(super) fn producer_command(record: &Value) -> (String, Vec<String>, String, i32, String) {
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

pub(super) fn producer_baseline_worktree(root: &Path, base_commit: &str) -> PathBuf {
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

pub(super) fn remove_producer_baseline_worktree(root: &Path, path: &Path) {
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

pub(super) fn producer_baseline_git_value(
    root: &Path,
    args: &[&str],
) -> Result<String, &'static str> {
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
pub(super) fn producer_try_advisory_lock(file: &fs::File) -> Result<(), &'static str> {
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
pub(super) fn producer_try_advisory_lock(_file: &fs::File) -> Result<(), &'static str> {
    Ok(())
}

pub(super) fn producer_lock(root: &Path) -> fs::File {
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

pub(super) fn producer_scope_hash(
    root: &Path,
    project: &Value,
    module: &Value,
    module_id: &str,
) -> String {
    let source_hash = hash_module_paths(root, project, module, module_id, "owned_paths");
    let contract_hash = hash_module_paths(root, project, module, module_id, "contract_paths");
    let registry_binding = normalized_registry_binding(module, module_id);
    sha256(&canonical(&serde_json::json!({
        "module_id": module_id,
        "source_hash": source_hash,
        "contract_hash": contract_hash,
        "registry_binding": registry_binding
    })))
}

pub(super) fn producer_record_targets(
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

pub(super) fn producer_attempt_path(root: &Path, module_id: &str) -> PathBuf {
    root.join(".appsdk")
        .join("records")
        .join("attempts")
        .join(module_id)
        .join("producer-records.jsonl")
}

pub(super) fn producer_baseline_path(root: &Path, module_id: &str, baseline_id: &str) -> PathBuf {
    if !baseline_id.starts_with("baseline-")
        || baseline_id.len() != "baseline-".len() + 64
        || !baseline_id["baseline-".len()..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        fail("PRODUCER_BASELINE_ID_INVALID");
    }
    root.join(".appsdk")
        .join("records")
        .join("evidence")
        .join(module_id)
        .join(format!("{}.json", baseline_id))
}

pub(super) fn producer_validate_attempt_ledger(root: &Path, module_id: &str) {
    let target = producer_attempt_path(root, module_id);
    assert_no_symlink_components(root, &target, "producer_record_archive");
    let metadata = match fs::symlink_metadata(&target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return,
        Err(_) => fail("PRODUCER_RECORD_ARCHIVE_INVALID"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        fail("PRODUCER_RECORD_ARCHIVE_INVALID");
    }
    let contents =
        fs::read_to_string(&target).unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_INVALID"));
    if contents.is_empty() || !contents.ends_with('\n') {
        fail("PRODUCER_RECORD_ARCHIVE_INVALID");
    }
    let records_root = root.join(".appsdk").join("records");
    let expected_worktree_path = records_root
        .join(module_record_name("worktree-record", module_id))
        .strip_prefix(root)
        .unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_INVALID"))
        .to_string_lossy()
        .replace('\\', "/");
    let expected_reproduction_path = records_root
        .join(module_record_name("reproduction-record", module_id))
        .strip_prefix(root)
        .unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_INVALID"))
        .to_string_lossy()
        .replace('\\', "/");
    let mut seen_archive_ids = BTreeSet::new();
    for line in contents.lines() {
        if line.trim().is_empty() {
            fail("PRODUCER_RECORD_ARCHIVE_INVALID");
        }
        let existing: Value =
            serde_json::from_str(line).unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_INVALID"));
        if existing.get("schema_version").and_then(Value::as_u64) != Some(1)
            || existing.get("module_id").and_then(Value::as_str) != Some(module_id)
            || existing.get("result").and_then(Value::as_str) != Some("stale")
        {
            fail("PRODUCER_RECORD_ARCHIVE_INVALID");
        }
        let archive_id =
            producer_string(&existing, "/archive_id", "PRODUCER_RECORD_ARCHIVE_INVALID");
        if !seen_archive_ids.insert(archive_id.clone()) {
            fail("PRODUCER_RECORD_ARCHIVE_CONFLICT");
        }
        let records = existing
            .get("records")
            .and_then(Value::as_array)
            .filter(|records| records.len() == 3)
            .unwrap_or_else(|| fail("PRODUCER_RECORD_ARCHIVE_INVALID"));
        let record_hash =
            producer_string(&existing, "/record_hash", "PRODUCER_RECORD_ARCHIVE_INVALID");
        if record_hash != sha256(&canonical(&Value::Array(records.clone()))) {
            fail("PRODUCER_RECORD_ARCHIVE_INVALID");
        }
        let expected_archive_id = producer_stable_id(
            "producer-attempt",
            &serde_json::json!({"module_id": module_id, "record_hash": record_hash}),
        );
        if archive_id != expected_archive_id {
            fail("PRODUCER_RECORD_ARCHIVE_INVALID");
        }
        let archived_at = DateTime::parse_from_rfc3339(&producer_string(
            &existing,
            "/archived_at",
            "PRODUCER_RECORD_ARCHIVE_INVALID",
        ))
        .unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_INVALID"))
        .with_timezone(&Utc);
        if archived_at > Utc::now() {
            fail("PRODUCER_RECORD_ARCHIVE_INVALID");
        }

        let path_at = |index: usize| {
            records[index]
                .get("path")
                .and_then(Value::as_str)
                .filter(|path| !path.is_empty())
                .unwrap_or_else(|| fail("PRODUCER_RECORD_ARCHIVE_INVALID"))
        };
        if path_at(0) != expected_worktree_path || path_at(1) != expected_reproduction_path {
            fail("PRODUCER_RECORD_ARCHIVE_INVALID");
        }
        let worktree = records[0]
            .get("record")
            .filter(|record| record.is_object())
            .unwrap_or_else(|| fail("PRODUCER_RECORD_ARCHIVE_INVALID"));
        let reproduction = records[1]
            .get("record")
            .filter(|record| record.is_object())
            .unwrap_or_else(|| fail("PRODUCER_RECORD_ARCHIVE_INVALID"));
        let evidence = records[2]
            .get("record")
            .filter(|record| record.is_object())
            .unwrap_or_else(|| fail("PRODUCER_RECORD_ARCHIVE_INVALID"));
        for entry in records {
            let record = entry
                .get("record")
                .unwrap_or_else(|| fail("PRODUCER_RECORD_ARCHIVE_INVALID"));
            if let Some(record_json) = entry.get("record_json") {
                let record_json = record_json
                    .as_str()
                    .unwrap_or_else(|| fail("PRODUCER_RECORD_ARCHIVE_INVALID"));
                let parsed = serde_json::from_str::<Value>(record_json)
                    .unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_INVALID"));
                if parsed != *record {
                    fail("PRODUCER_RECORD_ARCHIVE_CONFLICT");
                }
            } else if !producer_legacy_record_is_unambiguous(record) {
                fail("PRODUCER_RECORD_ARCHIVE_INVALID");
            }
        }
        let evidence_id =
            producer_string(evidence, "/evidence_id", "PRODUCER_RECORD_ARCHIVE_INVALID");
        let expected_evidence_path = producer_baseline_path(root, module_id, &evidence_id)
            .strip_prefix(root)
            .unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_INVALID"))
            .to_string_lossy()
            .replace('\\', "/");
        if path_at(2) != expected_evidence_path
            || producer_string(worktree, "/module_id", "PRODUCER_RECORD_ARCHIVE_INVALID")
                != module_id
            || producer_string(
                reproduction,
                "/module_id",
                "PRODUCER_RECORD_ARCHIVE_INVALID",
            ) != module_id
            || producer_string(
                evidence,
                "/scope/module_id",
                "PRODUCER_RECORD_ARCHIVE_INVALID",
            ) != module_id
            || producer_string(
                reproduction,
                "/baseline_evidence_id",
                "PRODUCER_RECORD_ARCHIVE_INVALID",
            ) != evidence_id
            || producer_string(
                reproduction,
                "/worktree_id",
                "PRODUCER_RECORD_ARCHIVE_INVALID",
            ) != producer_string(worktree, "/worktree_id", "PRODUCER_RECORD_ARCHIVE_INVALID")
            || producer_issue(worktree, "/issue_id", "PRODUCER_RECORD_ARCHIVE_INVALID")
                != producer_issue(reproduction, "/issue_id", "PRODUCER_RECORD_ARCHIVE_INVALID")
            || producer_issue(worktree, "/issue_id", "PRODUCER_RECORD_ARCHIVE_INVALID")
                != producer_issue(evidence, "/issue_id", "PRODUCER_RECORD_ARCHIVE_INVALID")
            || producer_string(evidence, "/result", "PRODUCER_RECORD_ARCHIVE_INVALID") != "pass"
        {
            fail("PRODUCER_RECORD_ARCHIVE_INVALID");
        }
    }
}

pub(super) fn producer_archive_current_set(root: &Path, module_id: &str) -> bool {
    producer_validate_attempt_ledger(root, module_id);
    let records_root = root.join(".appsdk").join("records");
    let fixed_targets = [
        records_root.join(module_record_name("worktree-record", module_id)),
        records_root.join(module_record_name("reproduction-record", module_id)),
    ];
    let mut fixed = Vec::with_capacity(fixed_targets.len());
    let mut present = 0usize;
    for target in &fixed_targets {
        assert_no_symlink_components(root, target, "producer_record_archive");
        if let Some((record, record_json)) =
            producer_read_record_bytes_if_present(target, "PRODUCER_RECORD_ARCHIVE_INVALID")
        {
            present += 1;
            fixed.push((target.clone(), record, record_json));
        }
    }
    if present == 0 {
        return false;
    }
    if present != fixed_targets.len() {
        fail("PRODUCER_RECORD_SET_INCOMPLETE");
    }

    let baseline_id = producer_string(
        &fixed[1].1,
        "/baseline_evidence_id",
        "PRODUCER_RECORD_ARCHIVE_INVALID",
    );
    let baseline_target = producer_baseline_path(root, module_id, &baseline_id);
    assert_no_symlink_components(root, &baseline_target, "producer_record_archive");
    let (baseline, baseline_json) =
        producer_read_record_bytes_if_present(&baseline_target, "PRODUCER_RECORD_SET_INCOMPLETE")
            .unwrap_or_else(|| fail("PRODUCER_RECORD_SET_INCOMPLETE"));

    let records = vec![
        serde_json::json!({
            "path": fixed[0].0.strip_prefix(root).unwrap_or(&fixed[0].0).to_string_lossy(),
            "record": fixed[0].1,
            "record_json": fixed[0].2
        }),
        serde_json::json!({
            "path": fixed[1].0.strip_prefix(root).unwrap_or(&fixed[1].0).to_string_lossy(),
            "record": fixed[1].1,
            "record_json": fixed[1].2
        }),
        serde_json::json!({
            "path": baseline_target.strip_prefix(root).unwrap_or(&baseline_target).to_string_lossy(),
            "record": baseline,
            "record_json": baseline_json
        }),
    ];
    let record_hash = sha256(&canonical(&Value::Array(records.clone())));
    let archive_id = producer_stable_id(
        "producer-attempt",
        &serde_json::json!({"module_id": module_id, "record_hash": record_hash}),
    );
    let target = producer_attempt_path(root, module_id);
    assert_no_symlink_components(root, &target, "producer_record_archive");
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            fail("PRODUCER_RECORD_ARCHIVE_INVALID");
        }
    }
    if target.is_file() {
        let contents =
            fs::read_to_string(&target).unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_INVALID"));
        for line in contents.lines() {
            let existing: Value = serde_json::from_str(line)
                .unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_INVALID"));
            if existing.get("archive_id").and_then(Value::as_str) == Some(&archive_id) {
                if existing.get("record_hash").and_then(Value::as_str) != Some(&record_hash)
                    || existing.get("records") != Some(&Value::Array(records.clone()))
                {
                    fail("PRODUCER_RECORD_ARCHIVE_CONFLICT");
                }
                return true;
            }
        }
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_WRITE_FAILED"));
    }
    let envelope = serde_json::json!({
        "schema_version": 1,
        "archive_id": archive_id,
        "module_id": module_id,
        "result": "stale",
        "record_hash": record_hash,
        "records": records,
        "archived_at": Utc::now().to_rfc3339()
    });
    let mut line = serde_json::to_vec(&envelope)
        .unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_WRITE_FAILED"));
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&target)
        .unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_WRITE_FAILED"));
    file.write_all(&line)
        .and_then(|_| file.sync_all())
        .unwrap_or_else(|_| fail("PRODUCER_RECORD_ARCHIVE_WRITE_FAILED"));
    if let Some(parent) = target.parent() {
        if let Ok(file) = OpenOptions::new().read(true).open(parent) {
            let _ = file.sync_all();
        }
    }
    producer_validate_attempt_ledger(root, module_id);
    true
}

pub(super) fn producer_stable_id(prefix: &str, value: &Value) -> String {
    let digest = sha256(&canonical(value));
    format!(
        "{}-{}",
        prefix,
        digest.strip_prefix("sha256:").unwrap_or(&digest)
    )
}

pub(super) fn lifecycle_chain_review_identity(
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

pub(super) fn assert_lifecycle_chain_review_identity(review: &Value) {
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

pub(super) fn assert_lifecycle_chain_review_identity_or_frozen_legacy(
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

pub(super) fn producer_transaction_dir(root: &Path, module_id: &str) -> PathBuf {
    root.join(".appsdk")
        .join("transactions")
        .join(format!("producer-{}", module_id))
}
