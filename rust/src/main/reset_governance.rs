use super::*;

pub(super) fn pin_lock(root: &Path, binary: &Path) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_no_symlink_components(root, &root.join(".appsdk"), "appsdk_control");
    let mut project = read_project(root);
    let project_version =
        required_str(&project, "/sdk/version", "INVALID_SDK_CONTRACT").to_string();
    if !matches!(
        project_version.as_str(),
        "0.1.3"
            | "0.1.4"
            | "0.1.5"
            | "0.1.6"
            | "0.1.0007"
            | "0.1.0008"
            | "0.1.0009"
            | "0.1.0010"
            | "0.1.0011"
            | "0.1.0012"
            | "0.1.0013"
            | "0.1.0014"
            | "0.1.0015"
    ) {
        fail(format!(
            "UNSUPPORTED_SDK_MIGRATION:{}:{}",
            project_version, SDK_VERSION
        ));
    }
    let transition_contracts = preflight_current_transition_contracts(root, &project);
    let previous_bundle_digests = sdk_migration_bundle_witnesses(root);
    let binary = binary
        .canonicalize()
        .unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
    let bytes = fs::read(&binary).unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
    let digest = digest_bytes(&bytes);
    let running_binary = env::current_exe().unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
    if digest_bytes(&fs::read(running_binary).unwrap_or_else(|_| fail("SDK_BINARY_MISSING")))
        != digest
    {
        fail("SDK_PIN_BINARY_BUNDLE_MISMATCH");
    }
    reconcile_authoring_bundle_manifest(root);
    let original_version = project_version.clone();
    if matches!(original_version.as_str(), "0.1.3" | "0.1.4") {
        write_legacy_migration_step(root, &project_version);
        project["sdk"]["version"] = Value::String("0.1.5".into());
        write_project(root, &project);
    }
    if matches!(original_version.as_str(), "0.1.3" | "0.1.4" | "0.1.5") {
        let migrated_project = read_project(root);
        migrate_governance_maps(root, &migrated_project, "0.1.5-to-0.1.6");
        project = migrated_project;
        project["sdk"]["version"] = Value::String("0.1.6".into());
        write_project(root, &project);
    }
    for step in [
        "0.1.5-to-0.1.6",
        "0.1.6-to-0.1.0007",
        "0.1.0007-to-0.1.0008",
        "0.1.0008-to-0.1.0009",
        "0.1.0009-to-0.1.0010",
        "0.1.0010-to-0.1.0011",
        "0.1.0011-to-0.1.0012",
        "0.1.0012-to-0.1.0013",
        "0.1.0013-to-0.1.0014",
    ] {
        let manifest = sdk_map_migration_manifest(step);
        let source_matches = GOVERNANCE_MAP_NAMES.iter().all(|name| {
            file_sha256(&root.join(".appsdk/maps").join(name), "governance_map")
                == record_str(
                    sdk_map_migration_entry(&manifest, name),
                    "/source_digest",
                    "sdk-map-migration",
                )
        });
        let record_exists = sdk_map_migration_root(root, step)
            .join("record.json")
            .is_file();
        let legacy_last_step = step == "0.1.0010-to-0.1.0011"
            && !matches!(
                original_version.as_str(),
                "0.1.0011" | "0.1.0012" | "0.1.0013" | "0.1.0014" | "0.1.0015"
            );
        let previous_last_step = step == "0.1.0011-to-0.1.0012"
            && !matches!(
                original_version.as_str(),
                "0.1.0012" | "0.1.0013" | "0.1.0014" | "0.1.0015"
            );
        let latest_historical_step = step == "0.1.0012-to-0.1.0013"
            && !matches!(
                original_version.as_str(),
                "0.1.0013" | "0.1.0014" | "0.1.0015"
            );
        // `0.1.0013-to-0.1.0014` was the live step before `0.1.0015`. Keep it
        // materialized only for projects that still need it, and let the new
        // final step take over the current live binding below.
        let previous_live_step = step == "0.1.0013-to-0.1.0014"
            && !matches!(original_version.as_str(), "0.1.0014" | "0.1.0015");
        if record_exists
            || source_matches
            || legacy_last_step
            || previous_last_step
            || latest_historical_step
            || previous_live_step
        {
            let current_project = read_project(root);
            migrate_governance_maps(root, &current_project, step);
        }
    }
    let current_project = read_project(root);
    migrate_governance_maps(root, &current_project, "0.1.0014-to-0.1.0015");
    let migrated_project = read_project(root);
    install_current_record_contracts(root);
    install_current_transition_contracts(root, &transition_contracts);
    project = migrated_project;
    project["sdk"]["version"] = Value::String(SDK_VERSION.into());
    project["governance"]["record_contracts"] = Value::Array(
        CANONICAL_RECORD_CONTRACTS
            .iter()
            .map(|path| Value::String((*path).into()))
            .collect(),
    );
    let mut lock = serde_json::Map::new();
    lock.insert("sdk".into(), Value::String("appsdk".into()));
    lock.insert("version".into(), Value::String(SDK_VERSION.into()));
    lock.insert("digest".into(), Value::String(digest.clone()));
    lock.insert("compiler_digest".into(), Value::String(digest));
    lock.insert("bundle_digest".into(), Value::String(sdk_bundle_digest()));
    lock.insert(
        "bundle_manifest_digest".into(),
        Value::String(digest_bytes(SDK_BUNDLE_MANIFEST.as_bytes())),
    );
    lock.insert(
        "bundle_resources".into(),
        serde_json::from_str::<Value>(SDK_BUNDLE_MANIFEST)
            .unwrap_or_else(|_| fail("INVALID_SDK_BUNDLE"))
            .get("resources")
            .cloned()
            .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE")),
    );
    if let Some(previous_bundle_digest) = previous_bundle_digests.first() {
        lock.insert(
            "previous_bundle_digest".into(),
            Value::String(previous_bundle_digest.clone()),
        );
        lock.insert(
            "previous_bundle_digests".into(),
            Value::Array(
                previous_bundle_digests
                    .into_iter()
                    .map(Value::String)
                    .collect::<Vec<_>>(),
            ),
        );
    }
    write_sdk_witness(root, &binary);
    install_bundle_resources(root);
    lock.insert("binary_ref".into(), Value::String("project-sdk".into()));
    lock.insert(
        "contract_schema".into(),
        project
            .get("schema_version")
            .cloned()
            .unwrap_or_else(|| fail("UNSUPPORTED_PROJECT_SCHEMA")),
    );
    let lock_path = root.join(".appsdk/sdk.lock");
    if fs::symlink_metadata(&lock_path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_lock");
    }
    atomic_write_json(&lock_path, &Value::Object(lock), "SDK_LOCK_WRITE_FAILED");
    write_project(root, &project);
    println!("pinned {}", binary.display());
    if original_version != SDK_VERSION {
        println!("next appsdk init <project> to refresh the standard template reference");
        println!("then audit effective rules and CI/hooks; reuse covered authorization and keep Guidance optional");
    }
}

pub(super) fn reset_root_first_segment(relative: &str) -> &str {
    relative.split('/').next().unwrap_or(relative)
}

pub(super) fn reset_root_conflicts_with_reserved(relative: &str, case_insensitive: bool) -> bool {
    let protected = [
        ".appsdk",
        ".appsdk-control",
        ".git",
        ".agent-collab",
        "active",
        "protected",
        "business",
    ];
    let first = reset_root_first_segment(relative);
    protected.iter().any(|reserved| {
        if case_insensitive {
            first.eq_ignore_ascii_case(reserved)
        } else {
            first == *reserved
        }
    })
}

pub(super) fn reset_root_filesystem_is_case_insensitive(root: &Path) -> bool {
    // A governance reset always has `.appsdk` present when it reaches this
    // check. Comparing its canonical path with a case variant gives us the
    // root filesystem's actual behavior without creating probe files.
    match (
        fs::canonicalize(root.join(".appsdk")),
        fs::canonicalize(root.join(".APPSDK")),
    ) {
        (Ok(actual), Ok(variant)) => actual == variant,
        _ => false,
    }
}

pub(super) fn reset_generated_roots(root: &Path) -> Result<Vec<String>, String> {
    let project = project_file(root);
    if fs::symlink_metadata(&project)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("GOVERNANCE_PATH_SYMLINK:project".into());
    }
    let text = fs::read_to_string(&project)
        .map_err(|_| format!("PROJECT_CONTRACT_MISSING:{}", project.display()))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|_| "INVALID_PROJECT_CONTRACT".to_string())?;
    let roots = reset_transaction_generated_roots_with_current_baseline(
        &value,
        reset_root_filesystem_is_case_insensitive(root),
    )?;
    for relative in roots.iter().skip(1) {
        let path = root.join(relative);
        if fs::symlink_metadata(root)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err("GOVERNANCE_PATH_SYMLINK:generated_root".into());
        }
        let relative_path = path
            .strip_prefix(root)
            .map_err(|_| "GOVERNANCE_PATH_ESCAPE:generated_root".to_string())?;
        let mut current = root.to_path_buf();
        for component in relative_path.components() {
            current.push(component.as_os_str());
            if fs::symlink_metadata(&current)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                return Err("GOVERNANCE_PATH_SYMLINK:generated_root".into());
            }
        }
    }
    Ok(roots)
}

pub(super) fn reset_requires_clean_worktree(root: &Path, mode: ResetMode) -> Result<(), String> {
    let worktree_root = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or(""),
            "rev-parse",
            "--show-toplevel",
        ])
        .output()
        .map_err(|_| "RESET_GIT_WORKTREE_REQUIRED".to_string())?;
    if !worktree_root.status.success() {
        return Err("RESET_GIT_WORKTREE_REQUIRED".into());
    }
    let worktree_root = PathBuf::from(String::from_utf8_lossy(&worktree_root.stdout).trim());
    let status_root = if mode == ResetMode::DiscardLegacy {
        worktree_root.as_path()
    } else {
        root
    };
    let mut status_command = Command::new("git");
    status_command.args([
        "-C",
        status_root.to_str().unwrap_or(""),
        "status",
        "--porcelain=v1",
        "-z",
    ]);
    // `init --fresh` historically scoped cleanliness to the project root so a
    // nested project did not fail on unrelated parent changes. Preserve that
    // exact gate for fresh mode; the reset entry remains whole-worktree.
    if mode == ResetMode::FreshInit {
        status_command.args(["--", "."]);
    }
    let status = status_command
        .output()
        .map_err(|_| "RESET_GIT_WORKTREE_REQUIRED".to_string())?;
    if !status.status.success() {
        return Err("RESET_GIT_WORKTREE_REQUIRED".into());
    }
    let ignored_lock = if mode == ResetMode::DiscardLegacy {
        let lock_path = reset_transaction_lock_path(root);
        let lock_is_inside_worktree = fs::canonicalize(&lock_path)
            .ok()
            .zip(fs::canonicalize(&worktree_root).ok())
            .is_some_and(|(lock, worktree)| lock.starts_with(worktree));
        if lock_is_inside_worktree {
            let lock_status = Command::new("git")
                .args([
                    "-C",
                    worktree_root.to_str().unwrap_or(""),
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--untracked-files=all",
                    "--",
                    lock_path.to_str().unwrap_or(""),
                ])
                .output()
                .map_err(|_| "RESET_GIT_WORKTREE_REQUIRED".to_string())?;
            if !lock_status.status.success() {
                return Err("RESET_GIT_WORKTREE_REQUIRED".into());
            }
            lock_status.stdout
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let has_unexpected_dirty = status.stdout.split(|byte| *byte == b'\0').any(|record| {
        if record.is_empty() {
            return false;
        }
        ignored_lock
            .split(|byte| *byte == b'\0')
            .all(|ignored| ignored.is_empty() || ignored != record)
    });
    if has_unexpected_dirty {
        return Err("RESET_REQUIRES_CLEAN_WORKTREE".into());
    }
    Ok(())
}

pub(super) fn reset_governance(root: &Path, discard_legacy: bool) {
    reset_governance_internal(root, discard_legacy, ResetMode::DiscardLegacy)
        .unwrap_or_else(|error| fail(error));
}
