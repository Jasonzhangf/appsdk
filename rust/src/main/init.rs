use super::*;

pub(super) fn render_appsdk_gitignore(mut content: String) -> Result<String, String> {
    if let Some(begin) = content.find(APPSDK_GITIGNORE_BEGIN) {
        let end_start = begin + APPSDK_GITIGNORE_BEGIN.len();
        let end = content[end_start..]
            .find(APPSDK_GITIGNORE_END)
            .map(|offset| end_start + offset)
            .ok_or_else(|| "INVALID_APPSDK_GITIGNORE_BLOCK".to_string())?;
        let end_after = end + APPSDK_GITIGNORE_END.len();
        let mut updated = String::with_capacity(content.len());
        updated.push_str(&content[..begin]);
        updated.push_str(APPSDK_GITIGNORE_BLOCK);
        let suffix = &content[end_after..];
        if !suffix.trim().is_empty() {
            updated.push_str(suffix);
        }
        return Ok(updated);
    }
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.is_empty() {
        content.push('\n');
    }
    content.push_str(APPSDK_GITIGNORE_BLOCK);
    Ok(content)
}

pub(super) fn ensure_appsdk_gitignore(root: &Path) {
    let path = root.join(".gitignore");
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:gitignore");
    }
    let content = fs::read_to_string(&path).unwrap_or_default();
    let updated = render_appsdk_gitignore(content.clone()).unwrap_or_else(|error| fail(error));
    if content != updated {
        fs::write(path, updated).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    }
}

pub(super) fn ensure_governance_layout(root: &Path) {
    fs::create_dir_all(root.join(".appsdk")).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    for dir in [
        "playground/experiments",
        "active/lib",
        "protected/source",
        "protected/contracts",
        "protected/history",
        "generated",
        "tests/core",
        ".appsdk/records",
        ".appsdk-control",
    ] {
        fs::create_dir_all(root.join(dir)).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    }
    bootstrap_contracts(root);
    ensure_appsdk_gitignore(root);
}

pub(super) fn write_if_missing(root: &Path, relative: &str, content: &str) {
    let target = root.join(relative);
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(format!("GOVERNANCE_PATH_SYMLINK:{}", relative));
    }
    if target.exists() {
        return;
    }
    fs::write(target, content).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
}

pub(super) fn write_project_agent_contract(root: &Path) {
    write_if_missing(root, "AGENTS.md", PROJECT_AGENTS_TEMPLATE);
}

pub(super) fn write_current_sdk_lock(root: &Path) {
    let project = read_project(root);
    if project.pointer("/sdk/version").and_then(Value::as_str) != Some(SDK_VERSION) {
        return;
    }
    let target = root.join(".appsdk/sdk.lock");
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_lock");
    }
    let existing = if target.exists() {
        let text = fs::read_to_string(&target).unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
        let value =
            serde_json::from_str::<Value>(&text).unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
        if value.get("sdk").and_then(Value::as_str) != Some("appsdk")
            || value.get("version").and_then(Value::as_str) != Some(SDK_VERSION)
            || value.get("contract_schema") != project.get("schema_version")
        {
            fail("INVALID_SDK_LOCK");
        }
        Some(value)
    } else {
        None
    };
    let current_bundle_digest = sdk_bundle_digest();
    let mut lock = serde_json::Map::new();
    lock.insert("sdk".into(), Value::String("appsdk".into()));
    lock.insert("version".into(), Value::String(SDK_VERSION.into()));
    lock.insert(
        "bundle_digest".into(),
        Value::String(current_bundle_digest.clone()),
    );
    lock.insert(
        "bundle_manifest_digest".into(),
        Value::String(digest_bytes(SDK_BUNDLE_MANIFEST.as_bytes())),
    );
    lock.insert("bundle_resources".into(), sdk_bundle_manifest_resources());
    lock.insert(
        "contract_schema".into(),
        project
            .get("schema_version")
            .cloned()
            .unwrap_or_else(|| fail("UNSUPPORTED_PROJECT_SCHEMA")),
    );
    if let Some(existing) = existing.as_ref() {
        for key in ["digest", "compiler_digest"] {
            if let Some(value) = existing.get(key).and_then(Value::as_str) {
                if value.len() == 71
                    && value.starts_with("sha256:")
                    && value[7..].chars().all(|c| c.is_ascii_hexdigit())
                {
                    lock.insert(key.into(), Value::String(value.into()));
                }
            }
        }
        if let Some(value) = existing.get("binary_ref").and_then(Value::as_str) {
            lock.insert("binary_ref".into(), Value::String(value.into()));
        }
        let valid_bundle = |value: &str| {
            value.len() == 71
                && value.starts_with("sha256:")
                && value[7..].chars().all(|c| c.is_ascii_hexdigit())
                && value != current_bundle_digest
        };
        let mut witnesses = sdk_migration_bundle_witnesses(root);
        if let Some(existing_bundle) = existing
            .get("bundle_digest")
            .and_then(Value::as_str)
            .filter(|value| valid_bundle(value))
        {
            if !witnesses.iter().any(|known| known == existing_bundle) {
                witnesses.push(existing_bundle.to_string());
            }
        }
        if let Some(previous_bundle_digest) = witnesses.first() {
            lock.insert(
                "previous_bundle_digest".into(),
                Value::String(previous_bundle_digest.clone()),
            );
            lock.insert(
                "previous_bundle_digests".into(),
                Value::Array(witnesses.into_iter().map(Value::String).collect::<Vec<_>>()),
            );
        }
    }
    atomic_write_json(&target, &Value::Object(lock), "SDK_LOCK_WRITE_FAILED");
}

pub(super) fn install_standard_template_reference(root: &Path) {
    let target = root.join(".appsdk/templates/minimal/AGENTS.md");
    assert_no_symlink_components(root, &target, "guidance_standard_template");
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:guidance_standard_template");
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    }
    atomic_write_bytes(
        &target,
        PROJECT_AGENTS_TEMPLATE.as_bytes(),
        "GUIDANCE_STANDARD_TEMPLATE_WRITE_FAILED",
    );
}

pub(super) fn write_project_scaffold(root: &Path) {
    write_if_missing(
        root,
        ".appsdk/project.json",
        r#"{
  "schema_version": 1,
  "project_id": "change-me",
  "sdk": {"name": "appsdk", "version": "0.1.0007", "bundle_manifest": ".appsdk/contracts/sdk-bundle.manifest.json", "resource_record": ".appsdk/sdk-resources.json"},
  "lifecycle": {"stage": "draft"},
  "access": {"protected_paths": [".appsdk/**", "generated/**", "protected/source/**"]},
  "development_scenarios": {"manifest": ".appsdk/contracts/development-scenarios.manifest.json", "enabled": []},
  "guidance": {
    "enforcement": "advisory",
    "compiled_manifest": ".appsdk/guidance/compiled.json",
    "rule_sources": [
      {"source_id":"project-agents","kind":"agents","path":"AGENTS.md","required":false,"precedence":100},
      {"source_id":"appsdk-governance-skill","kind":"skill","path":".appsdk/skills/appsdk-project-governance/SKILL.md","contract_path":".appsdk/skills/appsdk-project-governance/appsdk-guidance.json","required":true,"precedence":200}
    ]
  },
  "governance": {
    "playground_root": "playground/experiments/**",
    "active_root": "active/lib/**",
    "protected_root": "protected/**",
    "generated_root": "generated/**",
    "active_kind": "immutable_consumable_library",
    "protected_kinds": ["source", "contracts", "history"],
    "generated_kinds": ["compiler_output", "indexes"],
    "freeze_requirements": ["git_clean", "source_commit_or_tag", "library_hash", "public_api_hash", "review_pass", "previous_active_immutable"],
    "promotion_requires": ["experiment_evidence", "architecture_review_pass", "unique_owner", "required_gates"],
    "runtime_forbidden_roots": ["playground/**", "generated/**"],
    "record_contracts": ["contracts/records/worktree-record.schema.json", "contracts/records/reproduction-record.schema.json", "contracts/records/evidence-record.schema.json", "contracts/records/fix-candidate-record.schema.json", "contracts/records/goal-clarification-record.schema.json", "contracts/records/review-record.schema.json", "contracts/records/effectiveness-record.schema.json", "contracts/records/pre-review-validation-record.schema.json", "contracts/records/collaboration-record.schema.json", "contracts/records/collaboration-index.schema.json", "contracts/records/merge-queue-record.schema.json", "contracts/records/merge-queue-state.schema.json", "contracts/records/integration-record.schema.json", "contracts/records/mainline-receipt-record.schema.json", "contracts/records/collab-live-closure-record.schema.json", "contracts/records/merge-record.schema.json", "contracts/records/promotion-record.schema.json", "contracts/records/regression-report.schema.json", "contracts/records/freeze-record.schema.json", "contracts/records/record-graph.contract.json"],
    "zone_transition_contract": "contracts/transitions/zone-transition.manifest.json",
    "playground_retention": "archive_then_remove",
    "debug_merge_comment_required": true
  },
  "lifecycles": {"issue": "open", "library": "draft", "source_snapshot": "mutable", "artifact": "generated"},
  "modules": [{"module_id":"app-core","stage":"source_implemented","owned_paths":["playground/experiments/**","protected/source/**","tests/core/**"],"source_owner":"app-core","active_artifact":"active/lib/app-core/**","generated_outputs":["generated/**"],"contract_paths":["contracts/records/**","contracts/transitions/**"],"dependency_modules":[],"build":{"program":"sh","args":["-c","mkdir -p generated/modules/app-core/lib && printf 'app-core placeholder\\n' > generated/modules/app-core/lib/app-core.placeholder"],"working_directory":"."},"artifact_paths":["app-core.placeholder"],"regression":{"required_before_freeze":true,"suite_id":"app-core-regression","command":{"program":"cargo","args":["test"],"working_directory":"."},"input_paths":["playground/experiments/**","tests/core/**"],"minimum_test_count":1,"allow_skipped":false,"ordinary_mode_after_freeze":"disabled","reenable_on":["source_change","contract_change","public_api_change","artifact_change","dependency_change"]}}]
}
"#,
    );
    write_if_missing(
        root,
        ".appsdk/goal.json",
        r#"{"goal_id":"goal-change-me","raw_request":"Describe the intended change before implementation.","understood_objective":"The objective will be restated and confirmed before admission.","acceptance_criteria":["The user-confirmed acceptance criteria are recorded before implementation."],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"received","confirmed_by":null,"confirmed_at":null,"created_at":"2026-01-01T00:00:00Z"}
"#,
    );
}

pub(super) fn assert_init_workspace_safe(workspace: &Path) {
    if fs::symlink_metadata(workspace)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail(format!("TARGET_SYMLINK:{}", workspace.display()));
    }
    if workspace.exists() && !workspace.is_dir() {
        fail(format!("TARGET_NOT_DIRECTORY:{}", workspace.display()));
    }
    for ancestor in workspace.ancestors() {
        if ancestor == Path::new("/tmp") || ancestor == Path::new("/var") {
            continue;
        }
        if fs::symlink_metadata(ancestor)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(format!("TARGET_PARENT_SYMLINK:{}", ancestor.display()));
        }
    }
}

pub(super) fn resolve_init_target(workspace: &Path, project_root: Option<&str>) -> PathBuf {
    assert_init_workspace_safe(workspace);
    let Some(project_root) = project_root else {
        return workspace.to_path_buf();
    };
    let relative = Path::new(project_root);
    if relative == Path::new(".") {
        return workspace.to_path_buf();
    }
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        fail("INVALID_PROJECT_ROOT");
    }
    workspace.join(relative)
}

pub(super) fn existing_init_target(workspace: &Path, project_root: Option<&str>) -> Option<PathBuf> {
    let relative = project_root.unwrap_or(".");
    let relative_path = Path::new(relative);
    if relative.is_empty()
        || relative_path.is_absolute()
        || (relative != "."
            && relative_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            }))
    {
        fail("INVALID_PROJECT_ROOT");
    }
    let root = if relative == "." {
        workspace.to_path_buf()
    } else {
        workspace.join(relative_path)
    };
    if !root.join(".appsdk/project.json").is_file() {
        return None;
    }
    assert_no_symlink_components(workspace, &root, "existing_init_project");
    Some(root)
}

pub(super) fn existing_collab_control_target(workspace: &Path, project_root: Option<&str>) -> Option<PathBuf> {
    let relative = project_root.unwrap_or(".");
    let relative_path = Path::new(relative);
    if relative.is_empty()
        || relative_path.is_absolute()
        || (relative != "."
            && relative_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            }))
    {
        fail("INVALID_PROJECT_ROOT");
    }
    let root = if relative == "." {
        workspace.to_path_buf()
    } else {
        workspace.join(relative_path)
    };
    if !root.join(".agent-collab").is_dir() && !root.join(".appsdk-control").is_dir() {
        return None;
    }
    assert_no_symlink_components(workspace, &root, "existing_collab_control_project");
    Some(root)
}

pub(super) fn canonical_init_target(workspace: &Path, project_root: Option<&str>) -> PathBuf {
    let root = if let Some(project_root) = project_root {
        resolve_init_target(workspace, Some(project_root))
    } else {
        workspace.to_path_buf()
    };
    assert_init_workspace_safe(&root);
    if !root.exists() {
        return root.canonicalize().unwrap_or(root);
    }
    assert_no_symlink_components(workspace, &root, "existing_init_project");
    root.canonicalize()
        .unwrap_or_else(|_| fail(format!("PROJECT_ROOT_MISSING:{}", root.display())))
}

pub(super) fn fresh_init_recovery_pending(root: &Path) -> bool {
    let transaction_dir = reset_transaction_dir(root);
    match fs::symlink_metadata(&transaction_dir) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => fail(format!(
            "GOVERNANCE_RESET_RECOVERY_REQUIRED:{}:{error}",
            transaction_dir.display()
        )),
    }
}

pub(super) fn preparation_file(workspace: &Path) -> PathBuf {
    workspace.join(".appsdk-prepare.json")
}

pub(super) fn preparation_exists(workspace: &Path) -> bool {
    let path = preparation_file(workspace);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                fail("PREPARATION_SYMLINK");
            }
            true
        }
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(_) => fail("PREPARATION_INVALID"),
    }
}

pub(super) fn preparation_template() -> &'static str {
    r#"{
  "schema_version": 1,
  "preparation_id": "prepare-change-me",
  "status": "draft",
  "objective": "Describe the confirmed project or module change.",
  "change_kind": null,
  "project_root": null,
  "legacy_roots": [],
  "new_roots": [],
  "protected_roots": [],
  "runtime_forbidden_roots": [],
  "boundary": {
    "allowed_paths": [],
    "forbidden_paths": [],
    "payload_control_separation": "must be confirmed"
  },
  "acceptance_criteria": [],
  "non_goals": [],
  "assumptions": [],
  "questions": [
    {"question_id":"scope-kind","question":"Is this a new project, module refactor, project refactor, or debug task?","status":"open"},
    {"question_id":"project-root","question":"Which relative directory is the new AppSDK project root?","status":"open"},
    {"question_id":"legacy-boundary","question":"Which existing directories remain read-only and outside the new project?","status":"open"},
    {"question_id":"new-boundary","question":"Which directories may the new project create or modify?","status":"open"}
  ],
  "confirmed_by": null,
  "confirmed_at": null,
  "created_at": "2026-01-01T00:00:00Z"
}
"#
}

pub(super) fn read_preparation(workspace: &Path) -> Value {
    let path = preparation_file(workspace);
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("PREPARATION_SYMLINK");
    }
    let text = fs::read_to_string(path).unwrap_or_else(|_| fail("PREPARATION_MISSING"));
    let value: Value = serde_json::from_str(&text).unwrap_or_else(|_| fail("PREPARATION_INVALID"));
    if value.get("schema_version").and_then(Value::as_u64) != Some(1)
        || value.get("status").and_then(Value::as_str) != Some("confirmed")
        || value
            .get("objective")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .is_none()
        || value
            .get("change_kind")
            .and_then(Value::as_str)
            .filter(|value| {
                matches!(
                    *value,
                    "new_project" | "module_refactor" | "project_refactor" | "debug"
                )
            })
            .is_none()
        || value.get("project_root").and_then(Value::as_str).is_none()
        || value
            .get("confirmed_by")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .is_none()
        || value.get("confirmed_at").and_then(Value::as_str).is_none()
    {
        fail("PREPARATION_NOT_CONFIRMED");
    }
    value
}

pub(super) fn read_init_preparation(workspace: &Path) -> (Value, PathBuf) {
    for preparation_workspace in workspace.ancestors() {
        assert_init_workspace_safe(preparation_workspace);
        if !preparation_exists(preparation_workspace) {
            continue;
        }
        let preparation = read_preparation(preparation_workspace);
        if preparation_workspace == workspace {
            return (preparation, preparation_workspace.to_path_buf());
        }
        let project_root = preparation
            .get("project_root")
            .and_then(Value::as_str)
            .unwrap_or_else(|| fail("PREPARATION_PROJECT_ROOT_MISSING"));
        if resolve_init_target(preparation_workspace, Some(project_root)) != workspace {
            fail("PREPARATION_PROJECT_ROOT_MISMATCH");
        }
        return (preparation, preparation_workspace.to_path_buf());
    }
    fail("PREPARATION_MISSING")
}

pub(super) fn prepare_project(workspace: &Path) {
    assert_init_workspace_safe(workspace);
    fs::create_dir_all(workspace).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    let path = preparation_file(workspace);
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("PREPARATION_SYMLINK");
    }
    if !path.exists() {
        fs::write(&path, preparation_template())
            .unwrap_or_else(|_| fail("PREPARATION_WRITE_FAILED"));
        println!("created {}", path.display());
    } else {
        let text = fs::read_to_string(&path).unwrap_or_else(|_| fail("PREPARATION_INVALID"));
        println!("{}", text);
    }
}

pub(super) fn collab_init_timeout() -> Duration {
    env::var("APPSDK_COLLAB_INIT_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1..=COLLAB_INIT_TIMEOUT_MS).contains(value))
        .map(Duration::from_millis)
        .unwrap_or(COLLAB_INIT_TIMEOUT)
}

pub(super) fn run_collab_init(root: &Path) -> Result<Output, String> {
    let mut command = Command::new("collab");
    command.arg("init").current_dir(root);
    run_goal_collab_command(command, collab_init_timeout())
}

pub(super) fn collab_init_error_can_recover_identity(detail: &str) -> bool {
    detail.lines().any(|line| {
        let line = line.trim().strip_prefix("collab: ").unwrap_or(line.trim());
        if matches!(
            line,
            "token mismatch: identity does not own this worker_id"
                | "persisted Collab identity has no registered runtime"
                | "identity has no registered runtime"
                | "identity has no registered runtime binding"
                | "RUNTIME_BINDING_REJECTED: persisted identity has no registered runtime"
        ) {
            return true;
        }
        line.strip_prefix("persisted Collab identity ")
            .and_then(|value| value.strip_suffix(" has no registered runtime"))
            .is_some_and(|worker_id| !worker_id.trim().is_empty())
    })
}

pub(super) fn collab_identity_recovery_root_error(root: &Path) -> Option<String> {
    let canonical_root = match fs::canonicalize(root) {
        Ok(root) => root,
        Err(error) => {
            return Some(format!("project_root_invalid:{}:{error}", root.display()));
        }
    };
    if is_linked_git_worktree(&canonical_root) {
        return Some(format!("linked_git_worktree:{}", canonical_root.display()));
    }
    let output = Command::new("git")
        .args([
            "-C",
            canonical_root.to_str().unwrap_or("."),
            "rev-parse",
            "--show-toplevel",
        ])
        .output();
    let Ok(output) = output else {
        return Some("vcs_adapter_unavailable".into());
    };
    if !output.status.success() {
        return Some(format!(
            "canonical_git_root_unavailable:{}",
            canonical_root.display()
        ));
    }
    let git_root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let canonical_git_root = match fs::canonicalize(&git_root) {
        Ok(root) => root,
        Err(error) => {
            return Some(format!(
                "canonical_git_root_invalid:{}:{error}",
                git_root.display()
            ));
        }
    };
    if !canonical_root.starts_with(&canonical_git_root) {
        return Some(format!(
            "project_root_outside_git_root:{}:{}",
            canonical_root.display(),
            canonical_git_root.display()
        ));
    }
    None
}

pub(super) fn recover_collab_peer_identity(root: &Path) -> Result<Output, String> {
    let mut command = Command::new("collab");
    command.args(["worker", "recover"]).current_dir(root);
    run_goal_collab_command(command, collab_init_timeout())
}

pub(super) fn validate_collab_tmux_init(value: &Value, canonical_root: &str) -> Result<(), String> {
    let runtime = value
        .get("runtime")
        .ok_or_else(|| "COLLAB_INIT_RUNTIME_MISSING".to_string())?;
    let selected = value
        .get("transport_selected")
        .ok_or_else(|| "COLLAB_INIT_TRANSPORT_MISSING".to_string())?;
    let runtime_endpoint = runtime
        .get("tmuxEndpoint")
        .filter(|endpoint| endpoint.is_object())
        .ok_or_else(|| "COLLAB_INIT_TMUX_ENDPOINT_MISSING".to_string())?;
    let selected_endpoint = selected
        .get("tmux_endpoint")
        .filter(|endpoint| endpoint.is_object())
        .ok_or_else(|| "COLLAB_INIT_TMUX_ENDPOINT_MISSING".to_string())?;
    if runtime.get("transport").and_then(Value::as_str) != Some("tmux")
        || selected.get("kind").and_then(Value::as_str) != Some("tmux")
        || runtime_endpoint != selected_endpoint
    {
        return Err("COLLAB_INIT_TMUX_RUNTIME_TRANSPORT_MISMATCH".into());
    }
    if runtime
        .get("runtimeId")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
        || runtime
            .get("appserverId")
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err("COLLAB_INIT_TMUX_RUNTIME_IDENTITY_MISSING".into());
    }
    if runtime.get("projectRoot").and_then(Value::as_str) != Some(canonical_root) {
        return Err("COLLAB_INIT_RUNTIME_ROOT_MISMATCH".into());
    }
    let endpoint_valid = ["socket_path", "tmux_session_id", "pane_id"]
        .iter()
        .all(|key| {
            runtime_endpoint[*key]
                .as_str()
                .is_some_and(|value| !value.trim().is_empty())
        })
        && ["server_pid", "pane_pid"]
            .iter()
            .all(|key| runtime_endpoint[*key].as_u64().is_some_and(|pid| pid > 0));
    let expected_session = runtime_endpoint["codex_session_id"]
        .as_str()
        .or_else(|| runtime_endpoint["tmux_session_id"].as_str());
    let expected_thread = runtime_endpoint["codex_thread_id"]
        .as_str()
        .or_else(|| runtime_endpoint["pane_id"].as_str());
    let has_capability = |object: &Value| {
        object
            .get("capabilities")
            .and_then(Value::as_array)
            .is_some_and(|capabilities| {
                capabilities
                    .iter()
                    .any(|capability| capability.as_str() == Some("send_message_to_pane"))
            })
    };
    if !endpoint_valid
        || selected["endpoint"].as_str() != runtime_endpoint["socket_path"].as_str()
        || selected["namespace"].as_str() != runtime_endpoint["tmux_session_id"].as_str()
        || selected["session_id"].as_str() != expected_session
        || selected["thread_id"].as_str() != expected_thread
        || !has_capability(runtime)
        || !has_capability(selected)
        || runtime
            .get("processId")
            .and_then(Value::as_u64)
            .is_none_or(|pid| pid == 0)
    {
        return Err("COLLAB_INIT_TMUX_RUNTIME_BINDING_INVALID".into());
    }
    Ok(())
}

pub(super) fn initialize_collab_peer(root: &Path) {
    let output = match run_collab_init(root) {
        Ok(output) => output,
        Err(error) if error == "GOAL_COLLAB_COMMAND_TIMEOUT" => {
            eprintln!("COLLAB_INIT_TIMEOUT: collab init did not finish within the bounded registration window; run collab worker recover from the canonical project root or retry appsdk init . after the daemon is reachable; shared collaboration unavailable; independent work may continue");
            return;
        }
        Err(error) if error == "GOAL_COLLAB_OUTPUT_DRAIN_TIMEOUT" => {
            eprintln!("COLLAB_INIT_OUTPUT_TIMEOUT: collab init exited but output did not drain within the bounded registration window; run collab worker recover from the canonical project root or retry appsdk init . after the daemon is reachable; shared collaboration unavailable; independent work may continue");
            return;
        }
        Err(error) => {
            eprintln!("COLLAB_INIT_UNAVAILABLE:{}; shared collaboration unavailable; independent work may continue", error);
            return;
        }
    };
    let output = if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        if !collab_init_error_can_recover_identity(&detail) {
            eprintln!("COLLAB_INIT_FAILED:{}; shared collaboration unavailable; independent work may continue", detail.trim());
            return;
        }
        if let Some(reason) = collab_identity_recovery_root_error(root) {
            eprintln!("COLLAB_INIT_RECOVER_SKIPPED_NON_CANONICAL_ROOT:{reason}; original={}; run collab worker recover from the canonical project main tree; shared collaboration unavailable; independent work may continue", detail.trim());
            return;
        }
        let recovery = match recover_collab_peer_identity(root) {
            Ok(recovery) => recovery,
            Err(error) if error == "GOAL_COLLAB_COMMAND_TIMEOUT" => {
                eprintln!("COLLAB_INIT_RECOVER_TIMEOUT: collab worker recover did not finish within the bounded registration window after collab init failed with {}; shared collaboration unavailable; independent work may continue", detail.trim());
                return;
            }
            Err(error) if error == "GOAL_COLLAB_OUTPUT_DRAIN_TIMEOUT" => {
                eprintln!("COLLAB_INIT_RECOVER_OUTPUT_TIMEOUT: collab worker recover exited but output did not drain after collab init failed with {}; shared collaboration unavailable; independent work may continue", detail.trim());
                return;
            }
            Err(error) => {
                eprintln!("COLLAB_INIT_RECOVER_UNAVAILABLE:{error}; original={}; shared collaboration unavailable; independent work may continue", detail.trim());
                return;
            }
        };
        if !recovery.status.success() {
            eprintln!("COLLAB_INIT_RECOVER_FAILED:{}; original={}; shared collaboration unavailable; independent work may continue", String::from_utf8_lossy(&recovery.stderr).trim(), detail.trim());
            return;
        }
        match run_collab_init(root) {
            Ok(retry) if retry.status.success() => retry,
            Ok(retry) => {
                eprintln!("COLLAB_INIT_FAILED_AFTER_RECOVER:{}; original={}; shared collaboration unavailable; independent work may continue", String::from_utf8_lossy(&retry.stderr).trim(), detail.trim());
                return;
            }
            Err(error) => {
                eprintln!("COLLAB_INIT_RETRY_UNAVAILABLE:{error}; original={}; shared collaboration unavailable; independent work may continue", detail.trim());
                return;
            }
        }
    } else {
        output
    };
    let result = String::from_utf8_lossy(&output.stdout);
    let result = result.trim();
    if result.is_empty() {
        eprintln!(
            "COLLAB_INIT_INVALID_RESPONSE:empty stdout; shared collaboration unavailable; independent work may continue"
        );
        return;
    }
    match serde_json::from_str::<Value>(result) {
        Ok(value) => {
            let transport = value.get("transport_selected").and_then(Value::as_object);
            let valid_transport = transport.is_some_and(|transport| {
                matches!(
                    transport.get("kind").and_then(Value::as_str),
                    Some("appserver" | "tmux")
                )
            });
            if value.get("ok").and_then(Value::as_bool) != Some(true) || !valid_transport {
                eprintln!(
                    "COLLAB_INIT_INVALID_RESPONSE:{result}; shared collaboration unavailable; independent work may continue"
                );
                return;
            }
            if value["transport_selected"]["kind"].as_str() == Some("tmux") {
                let canonical_root = match fs::canonicalize(root) {
                    Ok(root) => root,
                    Err(error) => {
                        eprintln!("COLLAB_INIT_RUNTIME_ROOT_INVALID:{error}; shared collaboration unavailable; independent work may continue");
                        return;
                    }
                };
                if let Err(error) =
                    validate_collab_tmux_init(&value, &canonical_root.to_string_lossy())
                {
                    eprintln!(
                        "{error}; shared collaboration unavailable; independent work may continue"
                    );
                    return;
                }
                println!(
                    "collab-channel {}",
                    serde_json::json!({
                        "runtime": value["runtime"],
                        "runtime_receipt": null,
                        "transport_selected": value["transport_selected"],
                        "independent_work_allowed": true
                    })
                );
                return;
            }
            let runtime = match value.get("runtime") {
                Some(runtime) => match serde_json::from_value::<global_registry::RuntimeIdentity>(
                    runtime.clone(),
                ) {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        eprintln!(
                            "COLLAB_INIT_RUNTIME_INVALID:{error}; shared collaboration unavailable; independent work may continue"
                        );
                        return;
                    }
                },
                None => {
                    eprintln!(
                        "COLLAB_INIT_RUNTIME_MISSING; shared collaboration unavailable; independent work may continue"
                    );
                    return;
                }
            };
            let canonical_root = match fs::canonicalize(root) {
                Ok(root) => root,
                Err(error) => {
                    eprintln!(
                        "COLLAB_INIT_RUNTIME_ROOT_INVALID:{error}; shared collaboration unavailable; independent work may continue"
                    );
                    return;
                }
            };
            let canonical_root_text = canonical_root.to_string_lossy();
            if runtime.project_root != canonical_root_text {
                eprintln!(
                    "COLLAB_INIT_RUNTIME_ROOT_MISMATCH:expected={canonical_root_text};observed={}; shared collaboration unavailable; independent work may continue",
                    runtime.project_root
                );
                return;
            }
            let transport = &value["transport_selected"];
            let transport_endpoint = transport.get("endpoint").and_then(Value::as_str);
            let transport_namespace = transport.get("namespace").and_then(Value::as_str);
            let transport_thread_id = transport
                .get("thread_id")
                .and_then(Value::as_str)
                .filter(|thread_id| !thread_id.trim().is_empty());
            if transport_thread_id.is_none() {
                eprintln!(
                    "COLLAB_INIT_APPSERVER_THREAD_MISSING; shared collaboration unavailable; independent work may continue"
                );
                return;
            }
            if transport_endpoint != Some(runtime.endpoint.as_str())
                || transport_namespace != Some(runtime.namespace.as_str())
            {
                eprintln!(
                    "COLLAB_INIT_TRANSPORT_RUNTIME_MISMATCH:runtime_endpoint={};runtime_namespace={};transport_endpoint={};transport_namespace={}; shared collaboration unavailable; independent work may continue",
                    runtime.endpoint,
                    runtime.namespace,
                    transport_endpoint.unwrap_or("<missing>"),
                    transport_namespace.unwrap_or("<missing>")
                );
                return;
            }
            let transport_capabilities = transport
                .get("capabilities")
                .and_then(Value::as_array)
                .map(|capabilities| {
                    capabilities
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default();
            let runtime_capabilities = runtime
                .capabilities
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            if !transport_capabilities.contains("send_message_to_thread")
                || !runtime_capabilities.contains("send_message_to_thread")
            {
                eprintln!(
                    "COLLAB_INIT_APPSERVER_CAPABILITY_MISSING:send_message_to_thread; shared collaboration unavailable; independent work may continue"
                );
                return;
            }
            let receipt = match global_registry::register_runtime(&runtime) {
                Ok(receipt) => receipt,
                Err(error) => {
                    eprintln!(
                        "COLLAB_RUNTIME_REGISTRATION_FAILED:{error}; shared collaboration unavailable; independent work may continue"
                    );
                    return;
                }
            };
            println!(
                "collab-channel {}",
                serde_json::json!({
                    "runtime": runtime,
                    "runtime_receipt": receipt,
                    "transport_selected": value["transport_selected"],
                    "independent_work_allowed": true
                })
            );
        }
        Err(error) => {
            eprintln!(
                "COLLAB_INIT_INVALID_RESPONSE:{error}; shared collaboration unavailable; independent work may continue"
            );
        }
    }
}

pub(super) fn try_register_global_project(root: &Path) {
    match global_registry::reserve_project(root, SDK_VERSION) {
        Ok(reservation) => match reservation.commit() {
            Ok(receipt) => println!(
                "appsdk-registration {}",
                serde_json::to_string(&global_registry::receipt_json(&receipt)).unwrap()
            ),
            Err(error) => eprintln!(
                "GLOBAL_PROJECT_REGISTRATION_PENDING:{error}; local governance epoch is complete; host registration is unavailable or deferred"
            ),
        },
        Err(error) => eprintln!(
            "GLOBAL_PROJECT_REGISTRATION_PENDING:{error}; local governance epoch is complete; host registration is unavailable or deferred"
        ),
    }
}

pub(super) fn reserve_global_project(root: &Path) -> global_registry::ProjectRegistrationReservation {
    // Project initialization is a short host-wide transaction, but several
    // projects may initialize at once.  Wait with bounded backoff for the
    // expected writer contention; malformed registry state and lock I/O
    // failures still fail immediately at the unique registry owner.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut delay = Duration::from_millis(10);
    loop {
        match global_registry::reserve_project(root, SDK_VERSION) {
            Ok(reservation) => return reservation,
            Err(error)
                if error.starts_with("GLOBAL_REGISTRY_BUSY:") && Instant::now() < deadline =>
            {
                thread::sleep(delay);
                delay = (delay + delay).min(Duration::from_millis(250));
            }
            Err(error) => fail(format!("GLOBAL_PROJECT_REGISTRATION_FAILED:{error}")),
        }
    }
}

pub(super) fn commit_global_project(reservation: global_registry::ProjectRegistrationReservation) {
    let receipt = reservation
        .commit()
        .unwrap_or_else(|error| fail(format!("GLOBAL_PROJECT_REGISTRATION_FAILED:{error}")));
    println!(
        "appsdk-registration {}",
        serde_json::to_string(&global_registry::receipt_json(&receipt)).unwrap()
    );
}

pub(super) fn init_project(root: &Path, fresh: bool, discard_legacy: bool) {
    if root.exists()
        && fs::symlink_metadata(root)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    {
        fail(format!("TARGET_SYMLINK:{}", root.display()));
    }
    assert_ordinary_init_canonical_project_main_tree(root, fresh);
    if fresh {
        if !discard_legacy {
            fail("INIT_FRESH_REQUIRES_DISCARD_LEGACY_CONFIRMATION");
        }
        if !root.is_dir()
            || (!root.join(".appsdk/project.json").is_file() && !fresh_init_recovery_pending(root))
        {
            fail("INIT_FRESH_REQUIRES_EXISTING_PROJECT");
        }
    }
    fs::create_dir_all(root).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    if fresh {
        reset_governance_internal(root, true, ResetMode::FreshInit)
            .unwrap_or_else(|error| fail(error));
        assert_fresh_project_contract_targets(root);
        try_register_global_project(root);
        if let Err(reason) = memory::initialize_project(root) {
            eprintln!("{}; optional project memory initialization skipped", reason);
        }
        println!("initialized fresh governance epoch {}", root.display());
        println!("next appsdk guide compile");
        println!(
            "then appsdk guide init --task <task-id> --mode <develop|debug> --module <module-id>"
        );
        return;
    }
    let fresh_governance = !root.join(".appsdk/project.json").is_file();
    let existing_project_needs_guidance = !fresh_governance
        && serde_json::from_str::<Value>(
            &fs::read_to_string(root.join(".appsdk/project.json"))
                .unwrap_or_else(|_| fail("INVALID_PROJECT")),
        )
        .unwrap_or_else(|_| fail("INVALID_PROJECT"))
        .get("guidance")
        .is_none();
    ensure_governance_layout(root);
    write_project_scaffold(root);
    if fresh_governance {
        write_project_agent_contract(root);
    }
    // `init` is also the supported idempotent SDK refresh entrypoint. The
    // Bundle owns `.appsdk/contracts`, `.appsdk/docs`, `.appsdk/skills`, and
    // the resource manifest. Project-owned maps, records, Active, Protected,
    // and root record contracts remain untouched.
    install_bundle_resources(root);
    write_current_sdk_lock(root);
    install_standard_template_reference(root);
    try_register_global_project(root);
    initialize_collab_peer(root);
    if let Err(reason) = memory::initialize_project(root) {
        eprintln!("{}; optional project memory initialization skipped", reason);
    }
    println!("initialized {}", root.display());
    if existing_project_needs_guidance {
        println!(
            "next appsdk guide init --task guidance-setup --mode bootstrap --module <module-id>"
        );
        println!("then read project documents and present GuidanceSetupProposal for user approval");
    } else if !fresh_governance {
        println!(
            "next appsdk guide init --task guidance-upgrade --mode bootstrap --module <module-id>"
        );
        println!(
            "then compare current project rules with the installed standard template and present a non-destructive GuidanceSetupProposal for user approval"
        );
    } else {
        println!("next appsdk guide compile");
        println!(
            "then appsdk guide init --task <task-id> --mode <develop|debug> --module <module-id>"
        );
    }
}
