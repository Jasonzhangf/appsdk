use super::*;

pub(super) fn producer_record_transaction_validate_marker(
    root: &Path,
    module_id: &str,
    input_hash: &str,
) -> Option<bool> {
    let transaction = producer_transaction_dir(root, module_id);
    let marker_path = transaction.join("marker.json");
    if !transaction.exists() || !marker_path.is_file() {
        return None;
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
        || marker
            .get("replace_current")
            .is_some_and(|value| !value.is_boolean())
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
    Some(
        marker
            .get("replace_current")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )
}

pub(super) fn producer_record_transaction_recover<F>(
    root: &Path,
    module_id: &str,
    input_hash: &str,
    replace_current: bool,
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
    if marker
        .get("replace_current")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        != replace_current
    {
        fail("PRODUCER_TRANSACTION_REPLACE_MODE_MISMATCH");
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
        let staging_valid = staging.is_file()
            && file_sha256(&staging, "lifecycle_producer_staging") == expected_hash;
        let bytes = if target.exists() {
            if file_sha256(&target, "lifecycle_producer_record") == expected_hash {
                fs::read(&target).unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_RECOVERY_FAILED"))
            } else {
                if !replace_current || !staging_valid {
                    fail("PRODUCER_TRANSACTION_TARGET_CONFLICT");
                }
                pending_links.push((staging.clone(), target.clone(), true));
                fs::read(&staging).unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_RECOVERY_FAILED"))
            }
        } else {
            if !staging_valid {
                fail("PRODUCER_TRANSACTION_STAGING_MISSING");
            }
            pending_links.push((staging.clone(), target.clone(), false));
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
    for (staging, target, replace) in pending_links {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_RECOVERY_FAILED"));
        }
        if replace {
            producer_replace_target_from_staging(
                root,
                &staging,
                &target,
                "PRODUCER_TRANSACTION_RECOVERY_FAILED",
            );
        } else {
            fs::hard_link(&staging, &target)
                .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_RECOVERY_FAILED"));
        }
    }
    fs::remove_dir_all(&transaction)
        .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_CLEANUP_FAILED"));
    Some(recovered)
}

pub(super) fn producer_durable_json(target: &Path, value: &Value, error: &str) {
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

pub(super) fn producer_replace_target_from_staging(
    root: &Path,
    staging: &Path,
    target: &Path,
    error: &str,
) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| fail("STAGING_NONCE_FAILED"))
        .as_nanos();
    let backup = target.with_extension(format!("replaced.{}.{}", std::process::id(), nonce));
    assert_no_symlink_components(root, &backup, "lifecycle_producer_backup");
    assert_no_symlink_components(root, staging, "lifecycle_producer_staging");
    assert_no_symlink_components(root, target, "lifecycle_producer_record");
    if backup.exists() {
        fail(error);
    }
    let had_target = target.exists();
    if had_target {
        fs::rename(target, &backup).unwrap_or_else(|_| fail(error));
    }
    if let Err(_) = fs::rename(staging, target) {
        if had_target {
            let _ = fs::rename(&backup, target);
        }
        fail(error);
    }
    if backup.exists() {
        fs::remove_file(&backup).unwrap_or_else(|_| fail(error));
    }
    if let Some(parent) = target.parent() {
        if let Ok(file) = OpenOptions::new().read(true).open(parent) {
            let _ = file.sync_all();
        }
    }
}

pub(super) fn producer_commit_records(
    root: &Path,
    module_id: &str,
    input_hash: &str,
    replace_current: bool,
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
            "replace_current": replace_current,
            "records": entries
        }),
        "PRODUCER_TRANSACTION_WRITE_FAILED",
    );
    for (index, (target, record)) in targets.iter().enumerate() {
        let staging = transaction.join(format!("record-{}.json", index));
        if target.exists() {
            let expected_hash =
                digest_bytes(&(serde_json::to_string_pretty(record).unwrap() + "\n").into_bytes());
            if file_sha256(target, "lifecycle_producer_record") == expected_hash {
                let _ = fs::remove_file(staging);
            } else if replace_current {
                producer_replace_target_from_staging(
                    root,
                    &staging,
                    target,
                    "PRODUCER_TRANSACTION_WRITE_FAILED",
                );
            } else {
                fail("PRODUCER_TRANSACTION_TARGET_CONFLICT");
            }
        } else {
            fs::hard_link(&staging, target)
                .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_WRITE_FAILED"));
            let _ = fs::remove_file(staging);
        }
    }
    fs::remove_dir_all(&transaction)
        .unwrap_or_else(|_| fail("PRODUCER_TRANSACTION_CLEANUP_FAILED"));
}

pub(super) fn assert_produced_record_shapes(
    root: &Path,
    targets: &[(PathBuf, Value)],
    module_id: &str,
) {
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
    assert_bug_tracker_triage_evidence(worktree, &issue_id, Some(root), true);
    assert_producer_goal_issue_binding(worktree, &issue_id);

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

pub(super) fn assert_recovered_record_bindings(
    root: &Path,
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
    goal_issue_binding: Option<(&str, &str)>,
    input_hashes: &[String],
    command: &Value,
    expected_status: i32,
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
        || goal_issue_binding.is_some_and(|(goal_issue_id, binding)| {
            worktree.get("goal_issue_id").and_then(Value::as_str) != Some(goal_issue_id)
                || worktree.get("goal_issue_binding").and_then(Value::as_str) != Some(binding)
        })
        || (goal_issue_binding.is_none()
            && (worktree.get("goal_issue_id").is_some()
                || worktree.get("goal_issue_binding").is_some()))
        || bug_triage.is_some_and(|expected| worktree.get("bug_triage") != Some(expected))
        || bug_triage.is_none() && worktree.get("bug_triage").is_some()
    {
        fail("PRODUCER_RECOVERY_WORKTREE_BINDING_MISMATCH");
    }
    assert_produced_record_shapes(root, targets, module_id);

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
    assert_evidence_record(
        evidence,
        baseline_id,
        EvidenceValidationMode::Current(Utc::now()),
    );
    let output_hash = producer_string(
        evidence,
        "/output_hash",
        "PRODUCER_RECOVERY_BASELINE_OUTPUT_INVALID",
    );
    if output_hash.len() != 71
        || !output_hash.starts_with("sha256:")
        || !output_hash[7..]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        fail("PRODUCER_RECOVERY_BASELINE_OUTPUT_INVALID");
    }
    let recorded_status = evidence
        .get("exit_status")
        .and_then(Value::as_i64)
        .filter(|status| (-255..=255).contains(status) && *status != 0)
        .unwrap_or_else(|| fail("PRODUCER_RECOVERY_BASELINE_STATUS_INVALID"));
    if recorded_status != i64::from(expected_status) {
        fail(format!(
            "PRODUCER_RECOVERY_BASELINE_STATUS_MISMATCH:expected={}:actual={}",
            expected_status, recorded_status
        ));
    }
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
        || evidence.get("producer")
            != Some(&serde_json::json!({
                "adapter": "appsdk",
                "identity": "appsdk-lifecycle-record-producer"
            }))
    {
        fail("PRODUCER_RECOVERY_BASELINE_BINDING_MISMATCH");
    }
}

pub(super) fn producer_read_record_if_present(target: &Path, error: &str) -> Option<Value> {
    let metadata = match fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error_value) if error_value.kind() == ErrorKind::NotFound => return None,
        Err(_) => fail(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        fail(error);
    }
    Some(
        serde_json::from_str(&fs::read_to_string(target).unwrap_or_else(|_| fail(error)))
            .unwrap_or_else(|_| fail(error)),
    )
}

pub(super) fn producer_read_record_bytes_if_present(
    target: &Path,
    error: &str,
) -> Option<(Value, String)> {
    let metadata = match fs::symlink_metadata(target) {
        Ok(metadata) => metadata,
        Err(error_value) if error_value.kind() == ErrorKind::NotFound => return None,
        Err(_) => fail(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        fail(error);
    }
    let text = fs::read_to_string(target).unwrap_or_else(|_| fail(error));
    let value: Value = serde_json::from_str(&text).unwrap_or_else(|_| fail(error));
    Some((value, text))
}

pub(super) fn producer_legacy_record_is_unambiguous(record: &Value) -> bool {
    let Ok(pretty) = serde_json::to_string_pretty(record) else {
        return false;
    };
    serde_json::from_str::<Value>(&format!("{pretty}\n"))
        .ok()
        .as_ref()
        == Some(record)
}

pub(super) fn producer_without_fields(value: &Value, fields: &[&str]) -> Value {
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        for field in fields {
            object.remove(*field);
        }
    }
    value
}

pub(super) fn producer_assert_reuse_match(
    existing: &Value,
    expected: &Value,
    ignored_fields: &[&str],
    error: &str,
) {
    if producer_without_fields(existing, ignored_fields)
        != producer_without_fields(expected, ignored_fields)
    {
        fail(error);
    }
}

pub(super) fn producer_try_reuse_records(
    root: &Path,
    module_id: &str,
    input: &Value,
    base_ref: &str,
    base_commit: &str,
    head_commit: &str,
    branch: &str,
    scope_hash: &str,
    worktree_id: &str,
    reproduction_id: &str,
    baseline_id: &str,
    goal_issue_binding: Option<(&str, &str)>,
    input_hashes: &[String],
    command: &Value,
    expected_status: i32,
    expected_error_token: &str,
) -> Option<Vec<(PathBuf, Value)>> {
    producer_validate_attempt_ledger(root, module_id);
    if producer_transaction_dir(root, module_id).exists() {
        return None;
    }
    let records_root = root.join(".appsdk").join("records");
    let targets = [
        records_root.join(module_record_name("worktree-record", module_id)),
        records_root.join(module_record_name("reproduction-record", module_id)),
        records_root
            .join("evidence")
            .join(module_id)
            .join(format!("{}.json", baseline_id)),
    ];
    let mut existing = Vec::with_capacity(targets.len());
    let mut saw_missing = false;
    for target in &targets {
        assert_no_symlink_components(root, target, "record_control");
        if let Some(record) =
            producer_read_record_if_present(target, "PRODUCER_RECORD_REUSE_TARGET_INVALID")
        {
            existing.push(record);
        } else {
            saw_missing = true;
        }
    }
    if saw_missing {
        if existing.is_empty() {
            return None;
        }
        if targets[0].is_file() && targets[1].is_file() && !targets[2].is_file() {
            // The fixed worktree/reproduction projections belong to the last
            // current attempt while this input has a new baseline identity.
            // Let the producer run the real baseline command and replace the
            // current set after archiving it; a partial fixed set remains a
            // hard error below.
            return None;
        }
        fail("PRODUCER_RECORD_SET_INCOMPLETE");
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
    let issue_id = producer_issue(worktree, "/issue_id", "INVALID_WORKTREE_RECORD");
    let bug_triage = worktree.get("bug_triage");
    let mut expected_worktree = worktree.clone();
    expected_worktree["worktree_id"] = Value::String(worktree_id.to_string());
    expected_worktree["module_id"] = Value::String(module_id.to_string());
    expected_worktree["base_ref"] = Value::String(base_ref.to_string());
    expected_worktree["base_commit"] = Value::String(base_commit.to_string());
    expected_worktree["branch"] = Value::String(branch.to_string());
    expected_worktree["head_commit"] = Value::String(head_commit.to_string());
    expected_worktree["initial_clean"] = Value::Bool(true);
    expected_worktree["final_clean"] = Value::Bool(true);
    expected_worktree["isolation_mode"] = Value::String("isolated_worktree".into());
    expected_worktree["scope_hash"] = Value::String(scope_hash.to_string());
    expected_worktree
        .as_object_mut()
        .unwrap()
        .remove("goal_issue_binding");
    if let Some((goal_issue_id, binding)) = goal_issue_binding {
        expected_worktree["goal_issue_id"] = Value::String(goal_issue_id.to_string());
        expected_worktree["goal_issue_binding"] = Value::String(binding.to_string());
    }
    if let Some(triage) = bug_triage {
        expected_worktree["bug_triage_query_binding"] =
            Value::String(bug_triage_binding(&issue_id, triage));
    }
    producer_assert_reuse_match(
        &existing[0],
        &expected_worktree,
        &["created_at"],
        "PRODUCER_REUSE_WORKTREE_MISMATCH",
    );
    assert_bug_tracker_triage_evidence(&existing[0], &issue_id, Some(root), true);
    assert_producer_goal_issue_binding(&existing[0], &issue_id);

    let mut expected_reproduction = reproduction.clone();
    expected_reproduction["reproduction_id"] = Value::String(reproduction_id.to_string());
    expected_reproduction["issue_id"] = Value::String(issue_id.clone());
    expected_reproduction["module_id"] = Value::String(module_id.to_string());
    expected_reproduction["worktree_id"] = Value::String(worktree_id.to_string());
    expected_reproduction["base_commit"] = Value::String(base_commit.to_string());
    expected_reproduction["input_hashes"] = serde_json::json!(input_hashes);
    expected_reproduction["baseline_evidence_id"] = Value::String(baseline_id.to_string());
    expected_reproduction["first_divergence"] =
        Value::String(format!("baseline_error_token:{}", expected_error_token));
    expected_reproduction["result"] = Value::String("reproduced".into());
    producer_assert_reuse_match(
        &existing[1],
        &expected_reproduction,
        &["created_at"],
        "PRODUCER_REUSE_REPRODUCTION_MISMATCH",
    );

    let mut expected_baseline = baseline.clone();
    expected_baseline["issue_id"] = Value::String(issue_id);
    expected_baseline["source_commit"] = Value::String(base_commit.to_string());
    expected_baseline["evidence_id"] = Value::String(baseline_id.to_string());
    expected_baseline["input_hashes"] = serde_json::json!(input_hashes);
    expected_baseline["producer"] = serde_json::json!({
        "adapter": "appsdk",
        "identity": "appsdk-lifecycle-record-producer"
    });
    expected_baseline["result"] = Value::String("pass".into());
    expected_baseline["command"] = command.clone();
    expected_baseline["exit_status"] = Value::Number(expected_status.into());
    expected_baseline["phase"] = Value::String("baseline_reproduction".into());
    expected_baseline["scope_hash"] = Value::String(scope_hash.to_string());
    producer_assert_reuse_match(
        &existing[2],
        &expected_baseline,
        &["created_at", "expires_at", "output_hash"],
        "PRODUCER_REUSE_BASELINE_MISMATCH",
    );
    if existing[2]
        .get("output_hash")
        .and_then(Value::as_str)
        .is_none_or(|hash| {
            hash.len() != 71
                || !hash.starts_with("sha256:")
                || !hash[7..]
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
        })
    {
        fail("PRODUCER_REUSE_BASELINE_OUTPUT_INVALID");
    }
    let now = Utc::now();
    let expires_at = DateTime::parse_from_rfc3339(&producer_string(
        &existing[2],
        "/expires_at",
        "PRODUCER_REUSE_BASELINE_INVALID",
    ))
    .unwrap_or_else(|_| fail("PRODUCER_REUSE_BASELINE_INVALID"))
    .with_timezone(&Utc);
    if expires_at <= now {
        return None;
    }
    assert_evidence_record(
        &existing[2],
        baseline_id,
        EvidenceValidationMode::Current(now),
    );
    for (record, name) in [
        (&existing[0], "worktree-record"),
        (&existing[1], "reproduction-record"),
        (&existing[2], baseline_id),
    ] {
        if record_time(record, name) > now {
            fail("PRODUCER_REUSE_RECORD_FUTURE");
        }
    }

    Some(targets.into_iter().zip(existing).collect())
}
