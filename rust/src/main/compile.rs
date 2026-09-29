use super::*;

pub(super) fn assert_sdk_lock(root: &Path, project: &Value) {
    let file = root.join(".appsdk").join("sdk.lock");
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_lock");
    }
    let lock: Value = serde_json::from_str(
        &fs::read_to_string(file).unwrap_or_else(|_| fail("MISSING_SDK_LOCK")),
    )
    .unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
    if lock.get("sdk").and_then(Value::as_str) != Some("appsdk")
        || lock.get("version").and_then(Value::as_str)
            != project.pointer("/sdk/version").and_then(Value::as_str)
        || lock.get("contract_schema") != project.get("schema_version")
    {
        fail("INVALID_SDK_LOCK");
    }
    for key in ["digest", "compiler_digest"] {
        if let Some(digest) = lock.get(key) {
            let digest = digest.as_str().unwrap_or("");
            if digest.len() != 71
                || !digest.starts_with("sha256:")
                || !digest[7..].chars().all(|c| c.is_ascii_hexdigit())
            {
                fail("INVALID_SDK_LOCK_DIGEST");
            }
        }
    }
    for key in [
        "bundle_digest",
        "bundle_manifest_digest",
        "previous_bundle_digest",
    ] {
        if let Some(digest) = lock.get(key) {
            let digest = digest.as_str().unwrap_or("");
            if digest.len() != 71
                || !digest.starts_with("sha256:")
                || !digest[7..].chars().all(|c| c.is_ascii_hexdigit())
            {
                fail("INVALID_SDK_BUNDLE_DIGEST");
            }
        }
    }
    if let Some(digests) = lock.get("previous_bundle_digests") {
        let digests = digests
            .as_array()
            .unwrap_or_else(|| fail("INVALID_SDK_BUNDLE_DIGEST"));
        if digests.iter().any(|digest| {
            digest
                .as_str()
                .is_none_or(|digest| !valid_bundle_digest(digest))
        }) {
            fail("INVALID_SDK_BUNDLE_DIGEST");
        }
    }
    if let Some(resources) = lock.get("bundle_resources") {
        if !resources.is_object() {
            fail("INVALID_SDK_BUNDLE_RESOURCES");
        }
    }
}

pub(super) fn build_artifact(project: &Value) -> Value {
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let compiled = modules
        .iter()
        .map(|module| {
            let mut output = serde_json::Map::new();
            for key in [
                "module_id",
                "stage",
                "owned_paths",
                "source_owner",
                "active_artifact",
                "generated_outputs",
            ] {
                output.insert(
                    key.into(),
                    module
                        .get(key)
                        .cloned()
                        .unwrap_or_else(|| fail(format!("INVALID_MODULE_SURFACES:{}", key))),
                );
            }
            if let Some(regression) = module.get("regression") {
                output.insert("regression".into(), regression.clone());
            }
            Value::Object(output)
        })
        .collect::<Vec<_>>();
    let mut artifact = serde_json::Map::new();
    artifact.insert("artifact_schema".into(), Value::from(1));
    artifact.insert(
        "project_id".into(),
        project
            .get("project_id")
            .cloned()
            .unwrap_or_else(|| fail("INVALID_PROJECT_ID")),
    );
    artifact.insert(
        "sdk".into(),
        project
            .get("sdk")
            .cloned()
            .unwrap_or_else(|| fail("INVALID_SDK_CONTRACT")),
    );
    artifact.insert("modules".into(), Value::Array(compiled));
    let unsigned = Value::Object(artifact.clone());
    artifact.insert(
        "artifact_hash".into(),
        Value::String(sha256(&canonical(&unsigned))),
    );
    Value::Object(artifact)
}

pub(super) fn write_artifact(root: &Path, project: &Value) -> Value {
    let artifact = build_artifact(project);
    write_artifact_value(root, project, &artifact);
    artifact
}

pub(super) fn generated_root(root: &Path, project: &Value) -> PathBuf {
    contract_root(root, project, "/governance/generated_root")
}

pub(super) fn contract_root(root: &Path, project: &Value, path: &str) -> PathBuf {
    let value = required_str(project, path, "INVALID_GOVERNANCE_CONTRACT");
    let relative = value.trim_end_matches("/**").trim_end_matches('/');
    let candidate = Path::new(relative);
    if relative.is_empty()
        || candidate.is_absolute()
        || candidate.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        fail(format!("INVALID_GOVERNANCE_ROOT:{}", path));
    }
    let current = root.join(candidate);
    assert_no_symlink_components(root, &current, path);
    current
}

pub(super) fn assert_no_symlink_components(root: &Path, path: &Path, label: &str) {
    if fs::symlink_metadata(root)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(format!("GOVERNANCE_PATH_SYMLINK:{}", label));
    }
    let relative = path
        .strip_prefix(root)
        .unwrap_or_else(|_| fail(format!("GOVERNANCE_PATH_ESCAPE:{}", label)));
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        if fs::symlink_metadata(&current)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(format!("GOVERNANCE_PATH_SYMLINK:{}", label));
        }
    }
}

pub(super) fn safe_owned_path(root: &Path, relative: &str, label: &str) -> PathBuf {
    let trimmed = relative.trim_end_matches("/**").trim_end_matches('/');
    let path = Path::new(trimmed);
    if trimmed.is_empty() || path.is_absolute() {
        fail(format!("INVALID_OWNED_PATH:{}", label));
    }
    if trimmed != "."
        && path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        fail(format!("INVALID_OWNED_PATH:{}", label));
    }
    let full = root.join(path);
    assert_no_symlink_components(root, &full, label);
    full
}

pub(super) fn assert_vcs_clean(root: &Path, project: &Value, module_id: &str) {
    let probe = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "rev-parse",
            "--show-toplevel",
        ])
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !probe.status.success() {
        fail("VCS_ADAPTER_UNAVAILABLE");
    }
    let git_root = PathBuf::from(String::from_utf8_lossy(&probe.stdout).trim());
    let project_root = root
        .canonicalize()
        .unwrap_or_else(|_| fail("PROJECT_ROOT_MISSING"));
    let canonical_git_root = git_root
        .canonicalize()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    // The project may live in a subdirectory of a larger repository (for example
    // a V4 subproject inside a monorepo). Cleanliness must be scoped to the
    // project-relative prefix so unrelated sibling changes never block freeze.
    let prefix_probe = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "rev-parse",
            "--show-prefix",
        ])
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !prefix_probe.status.success() {
        fail("VCS_ADAPTER_UNAVAILABLE");
    }
    let _prefix = String::from_utf8_lossy(&prefix_probe.stdout)
        .trim()
        .to_string();
    if !project_root.starts_with(&canonical_git_root) {
        fail("VCS_PROJECT_ROOT_MISMATCH");
    }
    if !project
        .get("modules")
        .and_then(Value::as_array)
        .map(|modules| {
            modules
                .iter()
                .any(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or(false)
    {
        fail("MODULE_NOT_FOUND");
    }
    let mut vcs_scope = Command::new("git");
    vcs_scope.args([
        "-C",
        root.to_str().unwrap_or("."),
        "status",
        "--porcelain",
        "--",
    ]);
    vcs_scope.arg(project_root.to_str().unwrap_or("."));
    let output = vcs_scope
        .output()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    if !output.status.success() {
        fail("VCS_ADAPTER_FAILED");
    }
    let dirty = String::from_utf8_lossy(&output.stdout);
    for line in dirty.lines() {
        let paths = line.get(3..).unwrap_or("").trim();
        for path in paths.split(" -> ") {
            if !path.starts_with(".appsdk/transactions/") {
                fail("GIT_SCOPE_NOT_CLEAN");
            }
        }
    }
}

pub(super) fn assert_protected_not_ignored(root: &Path, archive: &Path) {
    let relative = archive
        .strip_prefix(root)
        .unwrap_or_else(|_| fail("GOVERNANCE_PATH_ESCAPE:protected_archive"));
    let status = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "check-ignore",
            "--no-index",
            "--quiet",
            "--",
        ])
        .arg(relative)
        .status()
        .unwrap_or_else(|_| fail("VCS_ADAPTER_UNAVAILABLE"));
    match status.code() {
        Some(1) => {}
        Some(0) => fail("PROTECTED_ARCHIVE_IGNORED"),
        _ => fail("VCS_ADAPTER_FAILED"),
    }
}

pub(super) fn copy_tree(source: &Path, target: &Path) {
    if fs::symlink_metadata(source)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("PROTECTED_ARCHIVE_SYMLINK");
    }
    if source.is_dir() {
        fs::create_dir_all(target).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
        for entry in fs::read_dir(source).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED")) {
            let entry = entry.unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
            if entry
                .file_type()
                .unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"))
                .is_symlink()
            {
                fail("PROTECTED_ARCHIVE_SYMLINK");
            }
            copy_tree(&entry.path(), &target.join(entry.file_name()));
        }
    } else if source.is_file() {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
        }
        fs::copy(source, target).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
    }
}

pub(super) fn staging_path(root: &Path, project: &Value, module_id: &str) -> PathBuf {
    generated_root(root, project)
        .join("active-publish")
        .join(format!("{}.{}", module_id, std::process::id()))
}

pub(super) fn module_generated_dir(root: &Path, project: &Value, module_id: &str) -> PathBuf {
    generated_root(root, project)
        .join("modules")
        .join(module_id)
}

pub(super) fn module_artifact_file(root: &Path, project: &Value, module_id: &str) -> PathBuf {
    module_generated_dir(root, project, module_id).join("module.compiled.json")
}

pub(super) fn module_lib_root(root: &Path, project: &Value, module_id: &str) -> PathBuf {
    module_generated_dir(root, project, module_id).join("lib")
}

pub(super) fn safe_module_artifact_path(
    root: &Path,
    project: &Value,
    module_id: &str,
    relative: &str,
) -> PathBuf {
    let lib_root = module_lib_root(root, project, module_id);
    let candidate = Path::new(relative);
    if relative.is_empty()
        || candidate.is_absolute()
        || candidate.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        fail(format!("INVALID_MODULE_ARTIFACT_PATH:{}", module_id));
    }
    let project_target = root.join(candidate);
    let legacy_target = lib_root.join(candidate);
    assert_no_symlink_components(root, &project_target, "module_artifact");
    assert_no_symlink_components(root, &legacy_target, "module_artifact");
    // Current project contracts declare paths from the project root. Keep the
    // historical module-lib-relative form for existing generated contracts.
    let generated_root = required_str(
        project,
        "/governance/generated_root",
        "INVALID_GOVERNANCE_CONTRACT",
    )
    .trim_end_matches("/**")
    .trim_end_matches('/');
    let project_relative = relative.trim_end_matches('/');
    let project_declared = registry_path_matches(generated_root, project_relative)
        || project
            .get("modules")
            .and_then(Value::as_array)
            .and_then(|modules| {
                modules.iter().find(|module| {
                    module.get("module_id").and_then(Value::as_str) == Some(module_id)
                })
            })
            .and_then(|module| module.get("generated_outputs"))
            .and_then(Value::as_array)
            .is_some_and(|outputs| {
                outputs.iter().any(|output| {
                    output
                        .as_str()
                        .is_some_and(|pattern| registry_path_matches(pattern, project_relative))
                })
            });
    if project_declared {
        project_target
    } else {
        legacy_target
    }
}

pub(super) fn file_sha256(path: &Path, label: &str) -> String {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(format!("ARTIFACT_PATH_SYMLINK:{}", label));
    }
    let bytes = fs::read(path).unwrap_or_else(|_| fail(format!("ARTIFACT_PATH_MISSING:{}", label)));
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

pub(super) fn hash_tree(root: &Path, prefix: &Path, label: &str) -> String {
    if !root.exists() {
        fail(format!("HASH_TREE_MISSING:{}", label));
    }
    let mut files = Vec::new();
    collect_files(root, prefix, label, &mut files);
    files.sort();
    let mut hasher = Sha256::new();
    for (relative, hash) in files {
        hasher.update(relative.as_os_str().as_encoded_bytes());
        hasher.update([0u8]);
        hasher.update(hash.as_bytes());
        hasher.update([0u8]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

pub(super) fn collect_files(
    root: &Path,
    prefix: &Path,
    label: &str,
    files: &mut Vec<(PathBuf, String)>,
) {
    let entries =
        fs::read_dir(root).unwrap_or_else(|_| fail(format!("HASH_TREE_READ_FAILED:{}", label)));
    let mut entries = entries.collect::<Vec<_>>();
    entries.sort_by_key(|entry| {
        entry
            .as_ref()
            .map(|entry| entry.file_name())
            .unwrap_or_default()
    });
    for entry in entries {
        let entry = entry.unwrap_or_else(|_| fail(format!("HASH_TREE_READ_FAILED:{}", label)));
        let entry_type = entry
            .file_type()
            .unwrap_or_else(|_| fail(format!("HASH_TREE_READ_FAILED:{}", label)));
        let path = entry.path();
        // npm dependency trees are generated inputs; their .bin entries are
        // ordinary symlinks and must not contaminate source ownership hashes.
        let relative = path
            .strip_prefix(prefix)
            .unwrap_or_else(|_| fail(format!("HASH_TREE_PREFIX:{}", label)));
        if relative
            .components()
            .any(|component| component.as_os_str().to_str() == Some("node_modules"))
        {
            continue;
        }
        if entry_type.is_symlink() {
            fail(format!("HASH_TREE_SYMLINK:{}", label));
        }
        if path.is_dir() {
            collect_files(&path, prefix, label, files);
        } else if path.is_file() {
            let relative = path
                .strip_prefix(prefix)
                .unwrap_or_else(|_| fail(format!("HASH_TREE_PREFIX:{}", label)))
                .to_path_buf();
            files.push((relative, file_sha256(&path, label)));
        }
    }
}

pub(super) fn module_build_command(module: &Value, module_id: &str) -> Value {
    module
        .get("build")
        .cloned()
        .unwrap_or_else(|| fail(format!("INVALID_MODULE_BUILD_CONTRACT:{}", module_id)))
}

pub(super) fn run_module_build(root: &Path, module: &Value, module_id: &str) {
    let build = module_build_command(module, module_id);
    let program = build
        .get("program")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail(format!("INVALID_MODULE_BUILD_CONTRACT:{}", module_id)));
    let args = build
        .get("args")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            fail(format!("INVALID_MODULE_BUILD_CONTRACT:{}", module_id))
                        })
                        .to_string()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| fail(format!("INVALID_MODULE_BUILD_CONTRACT:{}", module_id)));
    let working_directory = build
        .get("working_directory")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail(format!("INVALID_MODULE_BUILD_CONTRACT:{}", module_id)));
    let working = safe_owned_path(root, working_directory, "module_build_working_directory");
    let remap_root = root
        .canonicalize()
        .unwrap_or_else(|_| fail(format!("MODULE_BUILD_FAILED:{}", module_id)));
    let remap_flag = format!("--remap-path-prefix={}={}", remap_root.display(), ".");
    let mut command = Command::new(program);
    command.args(&args).current_dir(&working);
    let rustflags = match std::env::var("RUSTFLAGS") {
        Ok(existing) if !existing.trim().is_empty() => format!("{} {}", existing, remap_flag),
        _ => remap_flag,
    };
    command.env("RUSTFLAGS", rustflags);
    let output = command
        .output()
        .unwrap_or_else(|_| fail(format!("MODULE_BUILD_FAILED:{}", module_id)));
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        fail(format!("MODULE_BUILD_FAILED:{}", module_id));
    }
}

pub(super) fn hash_module_paths(
    root: &Path,
    _project: &Value,
    module: &Value,
    module_id: &str,
    key: &str,
) -> String {
    let paths = module
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail(format!("INVALID_MODULE_CONTRACT:{}:{}", module_id, key)));
    let mut hasher = Sha256::new();
    for path in paths {
        let relative = path
            .as_str()
            .unwrap_or_else(|| fail(format!("INVALID_MODULE_CONTRACT:{}:{}", module_id, key)));
        let safe = safe_owned_path(root, relative, "module_path_hash");
        let mut base = safe.clone();
        if relative.ends_with("/**") {
            base = safe_owned_path(
                root,
                relative.trim_end_matches("/**").trim_end_matches('/'),
                "module_path_hash",
            );
        }
        hasher.update(relative.as_bytes());
        hasher.update([0u8]);
        if safe.is_file() {
            hasher.update(file_sha256(&safe, "module_path").as_bytes());
        } else if safe.is_dir() || (relative.ends_with("/**") && base.exists()) {
            hasher.update(hash_tree(&base, &base, relative).as_bytes());
        } else {
            fail(format!("MODULE_PATH_MISSING:{}:{}", module_id, relative));
        }
        hasher.update([0u8]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

pub(super) fn module_dependency_hashes(
    root: &Path,
    project: &Value,
    module: &Value,
    module_id: &str,
) -> Vec<Value> {
    let dependencies = module
        .get("dependency_modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            fail(format!(
                "INVALID_MODULE_CONTRACT:{}:dependency_modules",
                module_id
            ))
        });
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let mut entries = Vec::new();
    for dependency in dependencies {
        let dependency_id = dependency
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                fail(format!(
                    "INVALID_MODULE_CONTRACT:{}:dependency_modules",
                    module_id
                ))
            });
        let dependency_module = modules
            .iter()
            .find(|module| module.get("module_id").and_then(Value::as_str) == Some(dependency_id))
            .unwrap_or_else(|| {
                fail(format!(
                    "MODULE_DEPENDENCY_NOT_FOUND:{}:{}",
                    module_id, dependency_id
                ))
            });
        let dependency_frozen =
            dependency_module.get("stage").and_then(Value::as_str) == Some("frozen");
        if module.get("stage").and_then(Value::as_str) == Some("frozen") && !dependency_frozen {
            fail(format!(
                "MODULE_DEPENDENCY_NOT_FROZEN:{}:{}",
                module_id, dependency_id
            ));
        }
        // Dependency-first declaration is also the recursion bound for freshness
        // checks invoked directly by review admission, before project verification.
        let position = |id: &str| {
            modules
                .iter()
                .position(|entry| entry.get("module_id").and_then(Value::as_str) == Some(id))
        };
        if position(dependency_id) >= position(module_id) {
            fail(format!(
                "MODULE_DEPENDENCY_ORDER:{}:{}",
                module_id, dependency_id
            ));
        }
        let artifact_file = module_artifact_file(root, project, dependency_id);
        if !artifact_file.is_file() {
            fail(format!(
                "MODULE_DEPENDENCY_ARTIFACT_MISSING:{}",
                dependency_id
            ));
        }
        let artifact: Value = serde_json::from_str(
            &fs::read_to_string(&artifact_file)
                .unwrap_or_else(|_| fail("MODULE_ARTIFACT_READ_FAILED")),
        )
        .unwrap_or_else(|_| fail("INVALID_MODULE_ARTIFACT"));
        module_artifact_matches_project(dependency_module, &artifact);
        let hash = record_str(&artifact, "/artifact_hash", "module-artifact");
        if !dependency_frozen {
            let current = build_module_artifact(root, project, dependency_module, dependency_id);
            if record_str(&current, "/artifact_hash", "dependency-artifact") != hash {
                fail(format!(
                    "MODULE_DEPENDENCY_ARTIFACT_STALE:{}:{}",
                    module_id, dependency_id
                ));
            }
        }
        entries.push(serde_json::json!({"module_id": dependency_id, "artifact_hash": hash}));
    }
    entries
}

pub(super) fn hash_module_artifacts(
    root: &Path,
    project: &Value,
    module: &Value,
    module_id: &str,
) -> Vec<Value> {
    let paths = module
        .get("artifact_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            fail(format!(
                "INVALID_MODULE_CONTRACT:{}:artifact_paths",
                module_id
            ))
        });
    let mut entries = Vec::new();
    for path in paths {
        let relative = path
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                fail(format!(
                    "INVALID_MODULE_CONTRACT:{}:artifact_paths",
                    module_id
                ))
            });
        let target = safe_module_artifact_path(root, project, module_id, relative);
        entries.push(serde_json::json!({
            "path": relative,
            "hash": file_sha256(&target, &format!("module_artifact:{}", module_id))
        }));
    }
    entries
}

pub(super) fn module_public_api_hash(artifact_entries: &[Value]) -> String {
    let mut hasher = Sha256::new();
    for entry in artifact_entries {
        hasher.update(
            entry
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("")
                .as_bytes(),
        );
        hasher.update([0u8]);
        hasher.update(
            entry
                .get("hash")
                .and_then(Value::as_str)
                .unwrap_or("")
                .as_bytes(),
        );
        hasher.update([0u8]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

pub(super) fn module_deployment_operations(module: &Value) -> Vec<&str> {
    let Some(value) = module.get("deployment_operations") else {
        // Existing service contracts retain both receipts until explicitly changed.
        return vec!["install", "restart"];
    };
    let values = value
        .as_array()
        .unwrap_or_else(|| fail("INVALID_DEPLOYMENT_OPERATIONS"));
    let mut operations = Vec::new();
    for value in values {
        let operation = value
            .as_str()
            .unwrap_or_else(|| fail("INVALID_DEPLOYMENT_OPERATIONS"));
        if !matches!(operation, "install" | "restart") || operations.contains(&operation) {
            fail("INVALID_DEPLOYMENT_OPERATIONS");
        }
        operations.push(operation);
    }
    operations
}

pub(super) fn assert_module_regression_contract(module: &Value, stage: &str, module_id: &str) {
    let Some(regression_value) = module.get("regression") else {
        if matches!(stage, "frozen" | "retired") {
            fail(format!("REGRESSION_CONTRACT_REQUIRED:{}", module_id));
        }
        return;
    };
    let regression = regression_value
        .as_object()
        .unwrap_or_else(|| fail(format!("INVALID_REGRESSION_CONTRACT:{}", module_id)));
    let required_before_freeze = regression
        .get("required_before_freeze")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| fail(format!("INVALID_REGRESSION_CONTRACT:{}", module_id)));
    if (!required_before_freeze && matches!(stage, "architecture_stable" | "frozen" | "retired"))
        || regression
            .get("suite_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        || regression
            .get("input_paths")
            .and_then(Value::as_array)
            .map(|values| {
                values.is_empty()
                    || values
                        .iter()
                        .any(|value| value.as_str().filter(|path| !path.is_empty()).is_none())
            })
            .unwrap_or(true)
        || regression
            .get("minimum_test_count")
            .and_then(Value::as_u64)
            .filter(|count| *count > 0)
            .is_none()
        || regression
            .get("allow_skipped")
            .and_then(Value::as_bool)
            .is_none()
        || regression
            .get("ordinary_mode_after_freeze")
            .and_then(Value::as_str)
            != Some("disabled")
        || regression
            .get("reenable_on")
            .and_then(Value::as_array)
            .map(|values| {
                [
                    "source_change",
                    "contract_change",
                    "public_api_change",
                    "artifact_change",
                    "dependency_change",
                ]
                .iter()
                .any(|required| !values.iter().any(|value| value.as_str() == Some(*required)))
            })
            .unwrap_or(true)
    {
        fail(format!("INVALID_REGRESSION_CONTRACT:{}", module_id));
    }
    let command = regression
        .get("command")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail(format!("INVALID_REGRESSION_CONTRACT:{}", module_id)));
    if command
        .get("program")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .is_none()
        || command
            .get("working_directory")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        || command
            .get("args")
            .and_then(Value::as_array)
            .map(|values| values.iter().any(|value| value.as_str().is_none()))
            .unwrap_or(true)
    {
        fail(format!("INVALID_REGRESSION_CONTRACT:{}", module_id));
    }
}

pub(super) fn build_module_artifact(
    root: &Path,
    project: &Value,
    module: &Value,
    module_id: &str,
) -> Value {
    let source_hash = hash_module_paths(root, project, module, module_id, "owned_paths");
    let contract_hash = hash_module_paths(root, project, module, module_id, "contract_paths");
    let dependency_hashes = module_dependency_hashes(root, project, module, module_id);
    let build_command = module_build_command(module, module_id);
    let artifact_entries = hash_module_artifacts(root, project, module, module_id);
    let public_api_hash = module_public_api_hash(&artifact_entries);
    let mut unsigned = serde_json::Map::new();
    if let Some(operations) = module.get("deployment_operations") {
        let _ = module_deployment_operations(module);
        unsigned.insert("deployment_operations".into(), operations.clone());
    }
    unsigned.insert("artifact_schema".into(), Value::from(1));
    unsigned.insert("module_id".into(), Value::String(module_id.into()));
    unsigned.insert(
        "stage".into(),
        module
            .get("stage")
            .cloned()
            .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT")),
    );
    unsigned.insert("source_hash".into(), Value::String(source_hash));
    unsigned.insert("contract_hash".into(), Value::String(contract_hash));
    unsigned.insert("dependency_hashes".into(), Value::Array(dependency_hashes));
    unsigned.insert("build".into(), build_command);
    unsigned.insert(
        "artifact_paths".into(),
        module
            .get("artifact_paths")
            .cloned()
            .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT")),
    );
    unsigned.insert("artifacts".into(), Value::Array(artifact_entries));
    unsigned.insert("public_api_hash".into(), Value::String(public_api_hash));
    let mut unsigned = unsigned;
    unsigned.remove("stage");
    let unsigned_value = Value::Object(unsigned);
    let artifact_hash = sha256(&canonical(&unsigned_value));
    let mut artifact = unsigned_value
        .as_object()
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
        .clone();
    artifact.insert(
        "stage".into(),
        module
            .get("stage")
            .cloned()
            .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT")),
    );
    artifact.insert("artifact_hash".into(), Value::String(artifact_hash));
    Value::Object(artifact)
}

pub(super) fn read_module_artifact(root: &Path, project: &Value, module_id: &str) -> Value {
    let file = module_artifact_file(root, project, module_id);
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:module_artifact");
    }
    serde_json::from_str(
        &fs::read_to_string(&file).unwrap_or_else(|_| fail("MISSING_RECORD:module-artifact")),
    )
    .unwrap_or_else(|_| fail("INVALID_MODULE_ARTIFACT"))
}

pub(super) fn read_historical_module_artifact(
    root: &Path,
    project: &Value,
    module: &Value,
    module_id: &str,
) -> Value {
    // A frozen checkout may still have its generated projection. Treat that
    // projection as authoritative for this admission attempt when it exists:
    // parse and validate it strictly, and never hide corruption by falling
    // through to an archive. Archive lookup is reserved for an actual missing
    // generated file.
    let current_file = module_artifact_file(root, project, module_id);
    assert_no_symlink_components(root, &current_file, "module_artifact");
    let current_artifact = match fs::symlink_metadata(&current_file) {
        Ok(_) => {
            let artifact = read_module_artifact(root, project, module_id);
            module_artifact_matches_project(module, &artifact);
            Some(artifact)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => fail("MISSING_RECORD:module-artifact"),
    };

    let freeze_name = freeze_record_name(module_id);
    let freeze = read_record(root, &freeze_name);
    let active_version = record_str(&freeze, "/active_version", &freeze_name);
    assert_version(active_version, "INVALID_ACTIVE_VERSION");
    let expected_hash = record_str(&freeze, "/library_hash", &freeze_name).to_string();
    let protected_root = contract_root(root, project, "/governance/protected_root");
    let current_archive = protected_root.join("history").join(module_id);
    let version_archive = protected_root
        .join("history-versions")
        .join(module_id)
        .join(active_version);
    // A versioned archive is the immutable source for the active publication.
    // It takes precedence over the compatibility `history/<module>` location.
    // Once the version directory exists, a damaged or mismatched artifact is
    // a hard failure; only an absent version directory permits the legacy
    // history fallback. This also makes two valid copies deterministic rather
    // than treating them as ambiguous.
    assert_no_symlink_components(root, &version_archive, "protected_version_history");
    let archive = match fs::symlink_metadata(&version_archive) {
        Ok(metadata) => {
            if !metadata.is_dir() {
                fail(format!("MODULE_ARTIFACT_HISTORY_MISSING:{}", module_id));
            }
            version_archive
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            assert_no_symlink_components(root, &current_archive, "protected_archive");
            match fs::symlink_metadata(&current_archive) {
                Ok(metadata) => {
                    if !metadata.is_dir() {
                        fail(format!("MODULE_ARTIFACT_HISTORY_MISSING:{}", module_id));
                    }
                    current_archive
                }
                Err(_) => fail(format!("MODULE_ARTIFACT_HISTORY_MISSING:{}", module_id)),
            }
        }
        Err(_) => fail(format!("MODULE_ARTIFACT_HISTORY_MISSING:{}", module_id)),
    };
    assert_no_symlink_components(root, &archive, "protected_archive");
    let artifact_file = archive.join("module-artifact.json");
    assert_no_symlink_components(root, &artifact_file, "module_artifact_history");
    if !artifact_file.is_file() {
        fail(format!("MODULE_ARTIFACT_HISTORY_MISSING:{}", module_id));
    }
    let artifact: Value = serde_json::from_str(
        &fs::read_to_string(&artifact_file)
            .unwrap_or_else(|_| fail(format!("MODULE_ARTIFACT_HISTORY_MISSING:{}", module_id))),
    )
    .unwrap_or_else(|_| fail("INVALID_MODULE_ARTIFACT"));
    if artifact.get("artifact_hash").and_then(Value::as_str) != Some(&expected_hash) {
        fail(format!(
            "MODULE_ARTIFACT_HISTORY_HASH_MISMATCH:{}",
            module_id
        ));
    }
    assert_protected_not_ignored(root, &archive);
    module_artifact_matches_project(module, &artifact);
    assert_protected_archive_matches(root, module, &artifact, &archive);
    if let Some(current_artifact) = current_artifact {
        if current_artifact != artifact {
            fail(format!(
                "FROZEN_REVIEW_GENERATED_ARTIFACT_MISMATCH:{}",
                module_id
            ));
        }
        current_artifact
    } else {
        artifact
    }
}

pub(super) fn write_module_artifact_value(
    root: &Path,
    project: &Value,
    module_id: &str,
    artifact: &Value,
) {
    let dir = module_generated_dir(root, project, module_id);
    fs::create_dir_all(&dir).unwrap_or_else(|_| fail("MODULE_ARTIFACT_WRITE_FAILED"));
    let target = dir.join("module.compiled.json");
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:module_artifact");
    }
    atomic_write_json(&target, artifact, "MODULE_ARTIFACT_WRITE_FAILED");
}

pub(super) fn module_artifact_matches_project(module: &Value, artifact: &Value) -> Value {
    let module_id = record_str(module, "/module_id", "module");
    if artifact.get("artifact_schema").and_then(Value::as_u64) != Some(1)
        || record_str(artifact, "/module_id", "module-artifact") != module_id
        || artifact.get("build") != module.get("build")
        || artifact.get("artifact_paths") != module.get("artifact_paths")
    {
        fail(format!("MODULE_ARTIFACT_MISMATCH:{}", module_id));
    }
    if artifact.get("deployment_operations") != module.get("deployment_operations") {
        fail("MODULE_DEPLOYMENT_CONTRACT_DRIFT");
    }
    let stored_hash = record_str(artifact, "/artifact_hash", "module-artifact");
    let mut unsigned = artifact.clone();
    unsigned
        .as_object_mut()
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
        .remove("artifact_hash");
    unsigned
        .as_object_mut()
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
        .remove("stage");
    if stored_hash != sha256(&canonical(&unsigned)) {
        fail(format!("MODULE_ARTIFACT_HASH_MISMATCH:{}", module_id));
    }
    artifact.clone()
}

/// Previous-active contract check: the already-published Active surface must
/// keep the same module identity and artifact surface, and must stay
/// self-consistent (its own signed hash still recomputes). The `build`
/// command is per-version reproduction metadata and may legitimately change
/// when a new version is opened (for example migrating a frozen consumer to a
/// resolver-managed link surface); it is therefore not compared against the
/// current module contract. The previous artifact remains hash-bound by its
/// own freeze record and by `version_base.base_artifact_hash`.
pub(super) fn previous_active_matches_module(module: &Value, artifact: &Value) {
    let module_id = record_str(module, "/module_id", "module");
    if artifact.get("artifact_schema").and_then(Value::as_u64) != Some(1)
        || record_str(artifact, "/module_id", "module-artifact") != module_id
        || artifact.get("artifact_paths") != module.get("artifact_paths")
    {
        fail(format!("MODULE_ARTIFACT_MISMATCH:{}", module_id));
    }
    let stored_hash = record_str(artifact, "/artifact_hash", "module-artifact");
    let mut unsigned = artifact.clone();
    unsigned
        .as_object_mut()
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
        .remove("artifact_hash");
    unsigned
        .as_object_mut()
        .unwrap_or_else(|| fail("INVALID_MODULE_ARTIFACT"))
        .remove("stage");
    if stored_hash != sha256(&canonical(&unsigned)) {
        fail(format!("MODULE_ARTIFACT_HASH_MISMATCH:{}", module_id));
    }
}

pub(super) fn compile_module_with_project(root: &Path, project: &Value, module_id: &str) -> Value {
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let index = modules
        .iter()
        .position(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    let module = &modules[index];
    if module.get("stage").and_then(Value::as_str) == Some("frozen") {
        fail(format!(
            "FROZEN_MODULE_REQUIRES_VERSIONED_ARTIFACT:{}",
            module_id
        ));
    }
    run_module_build(root, module, module_id);
    let mut module_with_stage = module.clone();
    let target_stage = module
        .get("stage")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("INVALID_MODULE_CONTRACT"))
        .to_string();
    module_with_stage["stage"] = Value::String(target_stage);
    let artifact = build_module_artifact(root, project, &module_with_stage, module_id);
    write_module_artifact_value(root, project, module_id, &artifact);
    println!("{}", serde_json::to_string_pretty(&artifact).unwrap());
    artifact
}

pub(super) fn compile_control_snapshot(root: &Path, project: &Value) -> CompileControlSnapshot {
    let manifest_path = project
        .pointer("/development_scenarios/manifest")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/development_scenarios/manifest"));
    let zone = contract_root(root, project, "/governance/zone_transition_contract");
    let mut paths = vec![
        (project_file(root), "project_contract"),
        (root.join(".appsdk/goal.json"), "goal"),
        (
            safe_owned_path(root, manifest_path, "development_scenarios"),
            "development_scenarios",
        ),
        (zone.clone(), "zone_transition_contract"),
        (
            zone.with_file_name("zone-transition.manifest.json"),
            "canonical_zone_contract",
        ),
    ];
    for declared in project
        .pointer("/governance/record_contracts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/governance/record_contracts"))
    {
        let relative = declared
            .as_str()
            .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/governance/record_contracts"));
        paths.push((
            safe_owned_path(root, relative, "record_contract"),
            "record_contract",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let files = paths
        .into_iter()
        .filter_map(|(path, label)| {
            if !seen.insert(path.clone()) {
                return None;
            }
            assert_no_symlink_components(root, &path, label);
            Some((path.clone(), label, file_sha256(&path, label)))
        })
        .collect();
    CompileControlSnapshot { files }
}

pub(super) fn assert_compile_control_snapshot(
    root: &Path,
    project: &Value,
    snapshot: &CompileControlSnapshot,
) {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    assert_compile_module_paths_safe(root, project);
    for (path, label, expected) in &snapshot.files {
        assert_no_symlink_components(root, path, label);
        if file_sha256(path, label) != *expected {
            fail("COMPILE_CONTROL_INPUT_DRIFT");
        }
    }
}

pub(super) fn assert_compile_module_paths_safe(root: &Path, project: &Value) {
    for module in project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"))
    {
        for (key, label) in [
            ("owned_paths", "module_owned_path"),
            ("generated_outputs", "module_generated_output"),
            ("contract_paths", "module_contract_path"),
        ] {
            for value in module
                .get(key)
                .and_then(Value::as_array)
                .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"))
            {
                let relative = value
                    .as_str()
                    .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"));
                safe_owned_path(root, relative, label);
            }
        }
        let active_artifact = module
            .get("active_artifact")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"));
        safe_owned_path(root, active_artifact, "module_active_artifact");
        let working_directory = module
            .pointer("/build/working_directory")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"));
        safe_owned_path(root, working_directory, "module_build_working_directory");
    }
}

pub(super) fn compile_module(root: &Path, module_id: &str) -> Value {
    assert_project_root_safe(root);
    assert_mutation_worktree(root);
    let project = read_project(root);
    assert_declared_contracts(root, &project);
    assert_goal_confirmed(root);
    assert_project_contract(root, &project);
    compile_module_with_project(root, &project, module_id)
}

pub(super) fn write_artifact_value(root: &Path, project: &Value, artifact: &Value) {
    let dir = generated_root(root, project);
    fs::create_dir_all(&dir).unwrap_or_else(|_| fail("ARTIFACT_WRITE_FAILED"));
    let target = dir.join("project.compiled.json");
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:artifact");
    }
    atomic_write_json(&target, artifact, "ARTIFACT_WRITE_FAILED");
}

pub(super) fn read_compiled_artifact(root: &Path, project: &Value) -> Value {
    let file = generated_root(root, project).join("project.compiled.json");
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:artifact");
    }
    serde_json::from_str(
        &fs::read_to_string(&file).unwrap_or_else(|_| fail("INVALID_ARTIFACT_SCHEMA")),
    )
    .unwrap_or_else(|_| fail("INVALID_ARTIFACT_SCHEMA"))
}

pub(super) fn assert_artifact_matches(project: &Value, artifact: &Value) {
    if artifact.get("artifact_schema").and_then(Value::as_u64) != Some(1)
        || artifact.get("project_id").and_then(Value::as_str)
            != project.get("project_id").and_then(Value::as_str)
        || artifact.pointer("/sdk/name").and_then(Value::as_str) != Some("appsdk")
        || artifact.pointer("/sdk/version") != project.pointer("/sdk/version")
        || artifact.get("modules").and_then(Value::as_array).is_none()
    {
        fail("INVALID_ARTIFACT_SCHEMA");
    }
    let stored_hash = record_str(artifact, "/artifact_hash", "artifact");
    let mut unsigned = artifact.clone();
    unsigned
        .as_object_mut()
        .unwrap_or_else(|| fail("INVALID_ARTIFACT_SCHEMA"))
        .remove("artifact_hash");
    if stored_hash != sha256(&canonical(&unsigned)) {
        fail("ARTIFACT_HASH_MISMATCH");
    }
    let project_modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"));
    let artifact_modules = artifact
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_ARTIFACT_SCHEMA"));
    if project_modules.len() != artifact_modules.len() {
        fail("ARTIFACT_MODULE_SET_MISMATCH");
    }
    let mut artifact_ids = std::collections::HashSet::new();
    for entry in artifact_modules {
        let id = record_str(entry, "/module_id", "artifact");
        if !artifact_ids.insert(id) {
            fail(format!("DUPLICATE_MODULE:{}", id));
        }
        let stage = record_str(entry, "/stage", "artifact");
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
            fail(format!("INVALID_MODULE_CONTRACT:{}", id));
        }
    }
    for module in project_modules {
        let module_id = record_str(module, "/module_id", "module");
        let compiled = artifact_modules
            .iter()
            .find(|entry| entry.get("module_id").and_then(Value::as_str) == Some(module_id))
            .unwrap_or_else(|| fail(format!("ARTIFACT_MODULE_MISMATCH:{}", module_id)));
        for key in [
            "stage",
            "source_owner",
            "active_artifact",
            "owned_paths",
            "generated_outputs",
            "regression",
        ] {
            if key == "stage"
                && module.get("version_base").is_some()
                && module.get("stage").and_then(Value::as_str) == Some("source_implemented")
                && compiled.get("stage").and_then(Value::as_str) == Some("frozen")
            {
                continue;
            }
            if key == "stage"
                && module.get("stage").and_then(Value::as_str) == Some("frozen")
                && compiled.get("stage").and_then(Value::as_str) == Some("architecture_stable")
            {
                continue;
            }
            if compiled.get(key) != module.get(key) {
                fail(format!("ARTIFACT_MODULE_MISMATCH:{}", module_id));
            }
        }
    }
}

pub(super) fn assert_compile_preconditions(
    root: &Path,
    project: &Value,
    changing_module: Option<&str>,
) {
    assert_project_contract(root, project);
    assert_goal_confirmed(root);
    assert_sdk_lock(root, project);
    let stage = required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT");
    if !matches!(
        stage,
        "contract_bound" | "compiled" | "controlled_verified" | "architecture_stable"
    ) {
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "error": "COMPILE_BLOCKED",
                "current_stage": stage,
                "required_stage": "contract_bound",
                "retry_allowed": false,
                "idempotent": true,
                "next": [
                    "confirm .appsdk/goal.json through the user-approved goal clarification flow",
                    "appsdk promote --to source_implemented",
                    "appsdk promote --to contract_bound",
                    "rerun appsdk compile once the project is contract_bound"
                ],
                "forbidden": [
                    "do not create generated/module artifacts by hand",
                    "do not edit lifecycle stage directly",
                    "do not retry compile before the stage changes"
                ]
            }))
            .unwrap()
        );
        std::process::exit(1);
    }
    if changing_module.is_none()
        && project
            .get("modules")
            .and_then(Value::as_array)
            .map(|modules| {
                modules.iter().all(|module| {
                    matches!(
                        module.get("stage").and_then(Value::as_str),
                        Some("frozen" | "retired")
                    )
                })
            })
            .unwrap_or(false)
    {
        fail("FROZEN_ARTIFACT_IMMUTABLE");
    }
}
