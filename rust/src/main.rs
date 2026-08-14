use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn fail(message: impl AsRef<str>) -> ! {
    eprintln!("{}", message.as_ref());
    std::process::exit(1);
}

fn write_embedded_contract(root: &Path, relative: &str, content: &str) {
    let target = root.join(relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    }
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:contract");
    }
    if target.exists() {
        return;
    }
    fs::write(target, content).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
}

fn bootstrap_contracts(root: &Path) {
    write_embedded_contract(
        root,
        ".appsdk/maps/resource-map.json",
        include_str!("../../contracts/maps/resource-map.json"),
    );
    write_embedded_contract(
        root,
        ".appsdk/maps/function-map.json",
        include_str!("../../contracts/maps/function-map.json"),
    );
    write_embedded_contract(
        root,
        ".appsdk/maps/mainline-call-map.json",
        include_str!("../../contracts/maps/mainline-call-map.json"),
    );
    write_embedded_contract(
        root,
        ".appsdk/maps/verification-map.json",
        include_str!("../../contracts/maps/verification-map.json"),
    );
    write_embedded_contract(
        root,
        "contracts/transitions/zone-transition-manifest.json",
        include_str!("../../contracts/transitions/zone-transition.manifest.json"),
    );
    write_embedded_contract(
        root,
        "contracts/transitions/zone-transition.manifest.json",
        include_str!("../../contracts/transitions/zone-transition.manifest.json"),
    );
    for (relative, content) in [
        (
            "contracts/records/evidence-record.schema.json",
            include_str!("../../contracts/records/evidence-record.schema.json"),
        ),
        (
            "contracts/records/goal-clarification-record.schema.json",
            include_str!("../../contracts/records/goal-clarification-record.schema.json"),
        ),
        (
            "contracts/records/review-record.schema.json",
            include_str!("../../contracts/records/review-record.schema.json"),
        ),
        (
            "contracts/records/promotion-record.schema.json",
            include_str!("../../contracts/records/promotion-record.schema.json"),
        ),
        (
            "contracts/records/freeze-record.schema.json",
            include_str!("../../contracts/records/freeze-record.schema.json"),
        ),
        (
            "contracts/records/record-graph.contract.json",
            include_str!("../../contracts/records/record-graph.contract.json"),
        ),
        (
            "contracts/lifecycle-state-machines.json",
            include_str!("../../contracts/lifecycle-state-machines.json"),
        ),
        (
            "contracts/lifecycle-state-machines.manifest.json",
            include_str!("../../contracts/lifecycle-state-machines.manifest.json"),
        ),
        (
            "contracts/goal-clarification-state-machine.json",
            include_str!("../../contracts/goal-clarification-state-machine.json"),
        ),
    ] {
        write_embedded_contract(root, relative, content);
    }
}

fn project_file(root: &Path) -> PathBuf {
    root.join(".appsdk").join("project.json")
}

fn read_project(root: &Path) -> Value {
    let file = project_file(root);
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:project");
    }
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|_| fail(format!("PROJECT_CONTRACT_MISSING:{}", file.display())));
    serde_json::from_str(&text).unwrap_or_else(|_| fail("INVALID_PROJECT_CONTRACT"))
}

fn assert_project_root_safe(root: &Path) {
    for ancestor in root.ancestors() {
        if ancestor == Path::new("/var") {
            continue;
        }
        if fs::symlink_metadata(ancestor)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(format!("PROJECT_ROOT_SYMLINK:{}", ancestor.display()));
        }
    }
    if fs::symlink_metadata(root)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("PROJECT_ROOT_SYMLINK");
    }
    let resolved = root
        .canonicalize()
        .unwrap_or_else(|_| fail("PROJECT_ROOT_MISSING"));
    for ancestor in resolved.ancestors() {
        if fs::symlink_metadata(ancestor)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(format!("PROJECT_ROOT_SYMLINK:{}", ancestor.display()));
        }
    }
    assert_no_symlink_components(root, &root.join(".appsdk"), "appsdk_control");
}

fn freeze_record_name(module_id: &str) -> String {
    format!("freeze-record-{}.json", module_id)
}

fn module_record_name(kind: &str, module_id: &str) -> String {
    format!("{}-{}.json", kind, module_id)
}

fn assert_declared_contracts(root: &Path, project: &Value, strict: bool) {
    let zone = contract_root(root, project, "/governance/zone_transition_contract");
    let canonical_project = project
        .pointer("/governance/zone_transition_contract")
        .and_then(Value::as_str)
        == Some("contracts/transitions/zone-transition-manifest.json")
        && project
            .pointer("/governance/record_contracts")
            .and_then(Value::as_array)
            .map(|values| {
                values.iter().any(|value| {
                    value.as_str() == Some("contracts/records/record-graph.contract.json")
                })
            })
            .unwrap_or(false);
    let strict = strict || canonical_project;
    if !canonical_project {
        fail("NON_CANONICAL_GOVERNANCE_CONTRACT");
    }
    let canonical_records = [
        "contracts/records/evidence-record.schema.json",
        "contracts/records/goal-clarification-record.schema.json",
        "contracts/records/review-record.schema.json",
        "contracts/records/promotion-record.schema.json",
        "contracts/records/freeze-record.schema.json",
        "contracts/records/record-graph.contract.json",
    ];
    let declared_records = project
        .pointer("/governance/record_contracts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/governance/record_contracts"));
    if declared_records.len() != canonical_records.len()
        || canonical_records.iter().any(|path| {
            !declared_records
                .iter()
                .any(|value| value.as_str() == Some(*path))
        })
    {
        fail("NON_CANONICAL_RECORD_CONTRACT_SET");
    }
    let zone_text = match fs::read_to_string(&zone) {
        Ok(text) => text,
        Err(_) if !strict => return,
        Err(_) => fail("DECLARED_ZONE_CONTRACT_MISSING"),
    };
    let zone_value: Value =
        serde_json::from_str(&zone_text).unwrap_or_else(|_| fail("INVALID_DECLARED_ZONE_CONTRACT"));
    if zone_value.pointer("/zones")
        != Some(&serde_json::json!([
            "playground",
            "active",
            "protected",
            "generated"
        ]))
        || zone_value
            .pointer("/transitions")
            .and_then(Value::as_array)
            .map(|v| v.len())
            .unwrap_or(0)
            < 16
    {
        fail("INVALID_DECLARED_ZONE_CONTRACT");
    }
    let canonical_path = zone.with_file_name("zone-transition.manifest.json");
    let canonical_path = if canonical_path == zone {
        zone.with_file_name("zone-transition.manifest.json")
    } else {
        canonical_path
    };
    let canonical_value: Value = serde_json::from_str(include_str!(
        "../../contracts/transitions/zone-transition.manifest.json"
    ))
    .unwrap_or_else(|_| fail("INVALID_CANONICAL_ZONE_CONTRACT"));
    if canonical_project && zone_value != canonical_value {
        fail("DECLARED_ZONE_CONTRACT_MISMATCH");
    }
    if strict && !canonical_path.exists() {
        fail("CANONICAL_ZONE_CONTRACT_MISSING");
    }
    for declared in project
        .pointer("/governance/record_contracts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/governance/record_contracts"))
    {
        let relative = declared
            .as_str()
            .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/governance/record_contracts"));
        let path = safe_owned_path(root, relative, "record_contract");
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(_) if !strict => return,
            Err(_) => fail("DECLARED_RECORD_CONTRACT_MISSING"),
        };
        let value: Value = serde_json::from_str(&text)
            .unwrap_or_else(|_| fail("INVALID_DECLARED_RECORD_CONTRACT"));
        let canonical_text = match relative {
            "contracts/records/evidence-record.schema.json" => {
                include_str!("../../contracts/records/evidence-record.schema.json")
            }
            "contracts/records/goal-clarification-record.schema.json" => {
                include_str!("../../contracts/records/goal-clarification-record.schema.json")
            }
            "contracts/records/review-record.schema.json" => {
                include_str!("../../contracts/records/review-record.schema.json")
            }
            "contracts/records/promotion-record.schema.json" => {
                include_str!("../../contracts/records/promotion-record.schema.json")
            }
            "contracts/records/freeze-record.schema.json" => {
                include_str!("../../contracts/records/freeze-record.schema.json")
            }
            "contracts/records/record-graph.contract.json" => {
                include_str!("../../contracts/records/record-graph.contract.json")
            }
            _ => fail("NON_CANONICAL_RECORD_CONTRACT_SET"),
        };
        let canonical_value: Value = serde_json::from_str(canonical_text)
            .unwrap_or_else(|_| fail("INVALID_CANONICAL_RECORD_CONTRACT"));
        if value != canonical_value {
            fail("DECLARED_RECORD_CONTRACT_MISMATCH");
        }
        if value.get("$schema").and_then(Value::as_str).is_none()
            || value.get("type").and_then(Value::as_str).is_none()
        {
            fail("INVALID_DECLARED_RECORD_CONTRACT");
        }
    }
}

fn assert_governance_maps(root: &Path) {
    for (name, key) in [
        ("resource-map.json", "resources"),
        ("function-map.json", "functions"),
        ("mainline-call-map.json", "edges"),
        ("verification-map.json", "gates"),
    ] {
        let file = root.join(".appsdk/maps").join(name);
        let value: Value = serde_json::from_str(
            &fs::read_to_string(&file)
                .unwrap_or_else(|_| fail(format!("MISSING_GOVERNANCE_MAP:{}", name))),
        )
        .unwrap_or_else(|_| fail(format!("INVALID_GOVERNANCE_MAP:{}", name)));
        if value.get("schema_version").and_then(Value::as_u64) != Some(1)
            || value
                .get(key)
                .and_then(Value::as_array)
                .map(|items| items.is_empty())
                .unwrap_or(true)
        {
            fail(format!("INVALID_GOVERNANCE_MAP:{}", name));
        }
    }
}

fn required_str<'a>(value: &'a Value, path: &str, error: &str) -> &'a str {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail(error))
}

fn assert_identifier(value: &str, error: &str) {
    if value.is_empty()
        || !value.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        || !value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        fail(error);
    }
}

fn canonical(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap(),
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|key| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap(),
                        canonical(&values[*key])
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn sha256(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn assert_goal_confirmed(root: &Path) {
    assert_goal_contract(root, true);
}

fn read_goal(root: &Path) -> Value {
    let file = root.join(".appsdk/goal.json");
    if fs::symlink_metadata(&file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:goal");
    }
    serde_json::from_str(
        &fs::read_to_string(file).unwrap_or_else(|_| fail("MISSING_GOAL_CLARIFICATION_RECORD")),
    )
    .unwrap_or_else(|_| fail("INVALID_GOAL_CLARIFICATION_RECORD"))
}

fn assert_goal_contract(root: &Path, require_confirmed: bool) {
    let goal = read_goal(root);
    for key in [
        "goal_id",
        "raw_request",
        "understood_objective",
        "created_at",
    ] {
        if goal
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            fail("INVALID_GOAL_CLARIFICATION_RECORD");
        }
    }
    for key in [
        "acceptance_criteria",
        "non_goals",
        "assumptions",
        "ambiguities",
        "questions",
    ] {
        if goal.get(key).and_then(Value::as_array).is_none() {
            fail("INVALID_GOAL_CLARIFICATION_RECORD");
        }
    }
    if goal["acceptance_criteria"].as_array().unwrap().is_empty()
        || goal["acceptance_criteria"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().map(|entry| entry.is_empty()).unwrap_or(true))
        || goal["non_goals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().is_none())
        || goal["assumptions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().is_none())
        || goal["ambiguities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str().is_none())
    {
        fail("INVALID_GOAL_CLARIFICATION_RECORD");
    }
    for question in goal["questions"].as_array().unwrap() {
        if question
            .get("question_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
            || question
                .get("question")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
            || !matches!(
                question.get("status").and_then(Value::as_str),
                Some("open" | "answered" | "not_required")
            )
            || question
                .get("answer")
                .map(|answer| !(answer.is_null() || answer.as_str().is_some()))
                .unwrap_or(false)
        {
            fail("INVALID_GOAL_CLARIFICATION_RECORD");
        }
    }
    let status = goal.get("status").and_then(Value::as_str).unwrap_or("");
    if !matches!(
        status,
        "received" | "parsed" | "clarification_pending" | "confirmed" | "admitted" | "superseded"
    ) {
        fail("INVALID_GOAL_CLARIFICATION_RECORD");
    }
    let open_questions = goal["questions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|question| question.get("status").and_then(Value::as_str) == Some("open"));
    if require_confirmed && open_questions {
        fail("GOAL_HAS_OPEN_QUESTIONS");
    }
    if require_confirmed {
        if !matches!(status, "confirmed" | "admitted") {
            fail(format!("GOAL_NOT_CONFIRMED:{}", status));
        }
        if goal
            .get("confirmed_by")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
            || goal
                .get("confirmed_at")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
        {
            fail("GOAL_CONFIRMATION_MISSING");
        }
        if status == "admitted" && goal.get("scope").and_then(Value::as_object).is_none() {
            fail("ADMITTED_GOAL_SCOPE_MISSING");
        }
    }
}

fn assert_sdk_lock(root: &Path, project: &Value, require_pinned: bool) {
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
        let digest = lock.get(key).and_then(Value::as_str).unwrap_or("");
        let placeholder = digest == "sha256:replace-with-compiled-sdk-digest"
            || digest == "sha256:replace-with-compiler-digest";
        if require_pinned && placeholder {
            fail("SDK_LOCK_NOT_PINNED");
        }
        if (!placeholder && digest.len() != 71)
            || !digest.starts_with("sha256:")
            || (!placeholder && !digest[7..].chars().all(|c| c.is_ascii_hexdigit()))
        {
            fail("INVALID_SDK_LOCK_DIGEST");
        }
    }
    if require_pinned {
        if lock.get("binary_ref").and_then(Value::as_str) != Some("project-sdk") {
            fail("INVALID_SDK_LOCK_BINARY_REF");
        }
        let executable = root.join(".appsdk/sdk.bin");
        let actual =
            digest_bytes(&fs::read(executable).unwrap_or_else(|_| fail("SDK_BINARY_MISSING")));
        if lock.get("digest").and_then(Value::as_str) != Some(actual.as_str())
            || lock.get("compiler_digest").and_then(Value::as_str) != Some(actual.as_str())
        {
            fail("SDK_BINARY_DIGEST_MISMATCH");
        }
        let running = std::env::current_exe().unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
        let running_digest =
            digest_bytes(&fs::read(running).unwrap_or_else(|_| fail("SDK_BINARY_MISSING")));
        if lock.get("digest").and_then(Value::as_str) != Some(running_digest.as_str()) {
            fail("RUNNING_SDK_BINARY_DIGEST_MISMATCH");
        }
    }
}

fn build_artifact(project: &Value) -> Value {
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

fn write_artifact(root: &Path, project: &Value) -> Value {
    let artifact = build_artifact(project);
    write_artifact_value(root, project, &artifact);
    artifact
}

fn generated_root(root: &Path, project: &Value) -> PathBuf {
    contract_root(root, project, "/governance/generated_root")
}

fn contract_root(root: &Path, project: &Value, path: &str) -> PathBuf {
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

fn assert_no_symlink_components(root: &Path, path: &Path, label: &str) {
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

fn safe_owned_path(root: &Path, relative: &str, label: &str) -> PathBuf {
    let trimmed = relative.trim_end_matches("/**").trim_end_matches('/');
    let path = Path::new(trimmed);
    if trimmed.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
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

fn assert_vcs_clean(root: &Path, project: &Value, module_id: &str) {
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
    if git_root.canonicalize().ok().as_ref() != Some(&project_root) {
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
    let output = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "status",
            "--porcelain",
            "--",
        ])
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

fn copy_tree(source: &Path, target: &Path) {
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

fn staging_path(root: &Path, project: &Value, module_id: &str) -> PathBuf {
    generated_root(root, project)
        .join("active-publish")
        .join(format!("{}.{}", module_id, std::process::id()))
}

fn write_artifact_value(root: &Path, project: &Value, artifact: &Value) {
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

fn read_compiled_artifact(root: &Path, project: &Value) -> Value {
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

fn assert_artifact_matches(project: &Value, artifact: &Value) {
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
        ] {
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

fn assert_compile_preconditions(root: &Path, project: &Value, changing_module: Option<&str>) {
    assert_project_contract(root, project);
    assert_goal_confirmed(root);
    assert_sdk_lock(root, project, true);
    let stage = required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT");
    if !matches!(
        stage,
        "contract_bound" | "compiled" | "controlled_verified" | "architecture_stable"
    ) {
        fail(format!("COMPILE_REQUIRES_CONTRACT_BOUND:{}", stage));
    }
    if changing_module.is_none()
        && project
            .get("modules")
            .and_then(Value::as_array)
            .map(|modules| {
                modules.iter().any(|module| {
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

fn assert_project_contract(root: &Path, project: &Value) {
    if project.get("schema_version").and_then(Value::as_u64) != Some(1)
        || project.get("project_id").and_then(Value::as_str).is_none()
        || project.pointer("/sdk/name").and_then(Value::as_str) != Some("appsdk")
        || project
            .pointer("/sdk/version")
            .and_then(Value::as_str)
            .is_none()
        || project
            .pointer("/lifecycle/stage")
            .and_then(Value::as_str)
            .is_none()
        || project
            .pointer("/access/protected_paths")
            .and_then(Value::as_array)
            .is_none()
    {
        fail("INVALID_PROJECT_CONTRACT");
    }
    assert_identifier(
        project
            .get("project_id")
            .and_then(Value::as_str)
            .unwrap_or(""),
        "INVALID_PROJECT_ID",
    );
    if project
        .pointer("/access/protected_paths")
        .and_then(Value::as_array)
        .map(|values| {
            values.is_empty()
                || values
                    .iter()
                    .any(|value| value.as_str().filter(|v| !v.is_empty()).is_none())
        })
        .unwrap_or(true)
    {
        fail("INVALID_PROJECT_CONTRACT:/access/protected_paths");
    }
    for path in [
        "/governance/playground_root",
        "/governance/active_root",
        "/governance/protected_root",
        "/governance/generated_root",
        "/governance/active_kind",
        "/governance/zone_transition_contract",
        "/governance/playground_retention",
    ] {
        if project.pointer(path).and_then(Value::as_str).is_none() {
            fail(format!("INVALID_PROJECT_CONTRACT:{}", path));
        }
    }
    for path in [
        "/governance/protected_kinds",
        "/governance/generated_kinds",
        "/governance/freeze_requirements",
        "/governance/promotion_requires",
        "/governance/runtime_forbidden_roots",
        "/governance/record_contracts",
    ] {
        if project.pointer(path).and_then(Value::as_array).is_none()
            || project
                .pointer(path)
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .any(|value| value.as_str().filter(|v| !v.is_empty()).is_none())
                })
                .unwrap_or(true)
        {
            fail(format!("INVALID_PROJECT_CONTRACT:{}", path));
        }
    }
    if project
        .pointer("/governance/active_kind")
        .and_then(Value::as_str)
        != Some("immutable_consumable_library")
    {
        fail("INVALID_PROJECT_CONTRACT:/governance/active_kind");
    }
    if project.pointer("/governance/debug_merge_comment_required") != Some(&Value::Bool(true))
        || !matches!(
            project
                .pointer("/governance/playground_retention")
                .and_then(Value::as_str),
            Some("archive_then_remove" | "archive_only")
        )
    {
        fail("INVALID_PROJECT_CONTRACT:/governance/lifecycle_controls");
    }
    let roots = [
        project
            .pointer("/governance/playground_root")
            .and_then(Value::as_str)
            .unwrap(),
        project
            .pointer("/governance/active_root")
            .and_then(Value::as_str)
            .unwrap(),
        project
            .pointer("/governance/protected_root")
            .and_then(Value::as_str)
            .unwrap(),
        project
            .pointer("/governance/generated_root")
            .and_then(Value::as_str)
            .unwrap(),
    ];
    for (index, left) in roots.iter().enumerate() {
        let left = left.trim_end_matches("/**").trim_end_matches('/');
        let left_path = Path::new(left);
        if left.is_empty()
            || left_path.is_absolute()
            || left_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            fail("INVALID_GOVERNANCE_ROOT");
        }
        for right in roots.iter().skip(index + 1) {
            let right = right.trim_end_matches("/**").trim_end_matches('/');
            if left == right
                || left.starts_with(&format!("{}/", right))
                || right.starts_with(&format!("{}/", left))
            {
                fail("OVERLAPPING_GOVERNANCE_ROOTS");
            }
        }
    }
    for path in [
        "/lifecycles/issue",
        "/lifecycles/library",
        "/lifecycles/source_snapshot",
        "/lifecycles/artifact",
    ] {
        if project.pointer(path).and_then(Value::as_str).is_none() {
            fail(format!("INVALID_PROJECT_CONTRACT:{}", path));
        }
    }
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_PROJECT_CONTRACT:/modules"));
    let mut ids = std::collections::HashSet::new();
    let mut owned_surfaces: Vec<(String, String)> = Vec::new();
    for module in modules {
        let id = module
            .get("module_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        assert_identifier(id, "INVALID_PROJECT_MODULE");
        if !ids.insert(id)
            || !matches!(
                module.get("stage").and_then(Value::as_str),
                Some(
                    "draft"
                        | "source_implemented"
                        | "contract_bound"
                        | "compiled"
                        | "controlled_verified"
                        | "architecture_stable"
                        | "frozen"
                        | "retired"
                )
            )
            || module
                .get("source_owner")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .is_none()
            || module.get("source_owner").and_then(Value::as_str) != Some(id)
            || module
                .get("active_artifact")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .is_none()
            || module
                .get("owned_paths")
                .and_then(Value::as_array)
                .map(|values| {
                    values.is_empty()
                        || values
                            .iter()
                            .any(|value| value.as_str().filter(|v| !v.is_empty()).is_none())
                })
                .unwrap_or(true)
            || module
                .get("generated_outputs")
                .and_then(Value::as_array)
                .map(|values| {
                    values.is_empty()
                        || values
                            .iter()
                            .any(|value| value.as_str().filter(|v| !v.is_empty()).is_none())
                })
                .unwrap_or(true)
        {
            fail(format!("INVALID_PROJECT_MODULE:{}", id));
        }
        for value in module["owned_paths"]
            .as_array()
            .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"))
        {
            let path = value
                .as_str()
                .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"))
                .trim_end_matches("/**")
                .trim_end_matches('/')
                .to_string();
            owned_surfaces.push((path, id.to_string()));
            safe_owned_path(
                root,
                value
                    .as_str()
                    .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE")),
                "module_owned_path",
            );
        }
        safe_owned_path(
            root,
            module["active_artifact"]
                .as_str()
                .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE")),
            "module_active_artifact",
        );
        owned_surfaces.push((
            module["active_artifact"]
                .as_str()
                .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"))
                .trim_end_matches("/**")
                .trim_end_matches('/')
                .to_string(),
            id.to_string(),
        ));
        for value in module["generated_outputs"]
            .as_array()
            .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"))
        {
            let path = value
                .as_str()
                .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE"))
                .trim_end_matches("/**")
                .trim_end_matches('/')
                .to_string();
            owned_surfaces.push((path, id.to_string()));
            safe_owned_path(
                root,
                value
                    .as_str()
                    .unwrap_or_else(|| fail("INVALID_PROJECT_MODULE")),
                "module_generated_output",
            );
        }
    }
    for (index, (left, left_owner)) in owned_surfaces.iter().enumerate() {
        for (right, right_owner) in owned_surfaces.iter().skip(index + 1) {
            if left_owner != right_owner
                && (left == right
                    || left.starts_with(&format!("{}/", right))
                    || right.starts_with(&format!("{}/", left)))
            {
                fail("OVERLAPPING_MODULE_OWNERSHIP");
            }
        }
    }
}

fn compile(root: &Path) {
    assert_project_root_safe(root);
    let project = read_project(root);
    assert_compile_preconditions(root, &project, None);
    assert_declared_contracts(root, &project, true);
    let artifact = write_artifact(root, &project);
    println!("{}", serde_json::to_string_pretty(&artifact).unwrap());
}

fn write_project(root: &Path, project: &Value) {
    assert_no_symlink_components(root, &root.join(".appsdk"), "appsdk_control");
    let target = project_file(root);
    if fs::symlink_metadata(&target)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:project");
    }
    atomic_write_json(&target, project, "PROJECT_WRITE_FAILED");
}

fn freeze_transaction_dir(root: &Path, module_id: &str) -> PathBuf {
    root.join(".appsdk")
        .join("transactions")
        .join(format!("freeze-{}", module_id))
}

fn recover_freeze_transaction(root: &Path, project: &Value, module_id: &str) -> bool {
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
    let archive = protected_root.join("history").join(module_id);
    let staging = protected_root.join("history").join(format!(
        ".{}.staging.{}",
        module_id,
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

fn atomic_write_json(target: &Path, value: &Value, error: &str) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| fail("STAGING_NONCE_FAILED"))
        .as_nanos();
    let staging = target.with_extension(format!("json.staging.{}.{}", std::process::id(), nonce));
    if fs::symlink_metadata(&staging)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:staging");
    }
    fs::write(
        &staging,
        serde_json::to_string_pretty(value).unwrap() + "\n",
    )
    .unwrap_or_else(|_| fail(error));
    fs::rename(&staging, target).unwrap_or_else(|_| fail(error));
}

fn read_record(root: &Path, name: &str) -> Value {
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

fn write_record(root: &Path, name: &str, record: &Value) {
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

fn record_str<'a>(record: &'a Value, path: &str, name: &str) -> &'a str {
    record
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail(format!("INVALID_RECORD:{}:{}", name, path)))
}

fn record_array<'a>(record: &'a Value, path: &str, name: &str) -> &'a Vec<Value> {
    record
        .pointer(path)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| fail(format!("INVALID_RECORD:{}:{}", name, path)))
}

fn assert_record_schema(evidence: &Value, review: &Value, promotion: &Value) {
    for (record, name, fields) in [
        (
            evidence,
            "evidence-record.json",
            &[
                "evidence_id",
                "issue_id",
                "experiment_id",
                "source_commit",
                "created_at",
                "expires_at",
                "scope_hash",
            ][..],
        ),
        (
            review,
            "review-record.json",
            &[
                "review_id",
                "issue_id",
                "promotion_id",
                "reviewed_commit",
                "reviewed_artifact_hash",
                "reviewed_scope_hash",
                "confidence_rationale",
                "created_at",
            ][..],
        ),
        (
            promotion,
            "promotion-record.json",
            &[
                "promotion_id",
                "issue_id",
                "experiment_id",
                "module_id",
                "base_commit",
                "source_commit",
                "new_active_version",
                "review_id",
                "root_cause",
                "design_id",
                "change_reason_comment",
                "playground_cleanup_record_id",
                "created_at",
            ][..],
        ),
    ] {
        for field in fields {
            record_str(record, &format!("/{}", field), name);
        }
    }
    for path in [
        "/kind",
        "/source_commit",
        "/result",
        "/created_at",
        "/expires_at",
        "/scope_hash",
    ] {
        record_str(evidence, path, "evidence-record.json");
    }
    for path in [
        "/reviewer/adapter",
        "/reviewer/identity",
        "/verdict",
        "/reviewed_commit",
        "/reviewed_artifact_hash",
        "/reviewed_scope_hash",
        "/confidence_rationale",
        "/created_at",
    ] {
        record_str(review, path, "review-record.json");
    }
    for path in [
        "/base_commit",
        "/new_active_version",
        "/review_id",
        "/change_set_id",
        "/compatibility_level",
        "/root_cause",
        "/design_id",
        "/change_reason_comment",
        "/playground_cleanup_record_id",
        "/created_at",
    ] {
        record_str(promotion, path, "promotion-record.json");
    }
    if !matches!(
        promotion.get("compatibility_level").and_then(Value::as_str),
        Some("compatible" | "migration_required" | "breaking")
    ) {
        fail("INVALID_PROMOTION_RECORD");
    }
    if !matches!(
        evidence.get("kind").and_then(Value::as_str),
        Some(
            "red_test"
                | "positive_test"
                | "negative_test"
                | "sample_replay"
                | "build"
                | "artifact"
                | "runtime"
                | "gate"
        )
    ) || evidence.get("result").and_then(Value::as_str).is_none()
        || evidence.get("result").and_then(Value::as_str) == Some("fail")
    {
        fail("INVALID_EVIDENCE_RECORD");
    }
    let scope = evidence
        .get("scope")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("INVALID_EVIDENCE_RECORD"));
    if scope
        .get("module_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        fail("INVALID_EVIDENCE_RECORD");
    }
    let producer = evidence
        .get("producer")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("INVALID_EVIDENCE_RECORD"));
    for key in ["adapter", "identity"] {
        if producer
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            fail("INVALID_EVIDENCE_RECORD");
        }
    }
    if evidence
        .get("input_hashes")
        .and_then(Value::as_array)
        .map(|values| values.iter().any(|value| value.as_str().is_none()))
        .unwrap_or(true)
    {
        fail("INVALID_EVIDENCE_RECORD");
    }
    let reviewer = review
        .get("reviewer")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("INVALID_REVIEW_RECORD"));
    for key in ["adapter", "identity"] {
        if reviewer
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            fail("INVALID_REVIEW_RECORD");
        }
    }
    if review
        .get("evidence_ids")
        .and_then(Value::as_array)
        .map(|values| values.is_empty() || values.iter().any(|value| value.as_str().is_none()))
        .unwrap_or(true)
    {
        fail("INVALID_REVIEW_RECORD");
    }
    if !promotion
        .get("previous_active_version")
        .map(|value| value.is_null() || value.as_str().is_some())
        .unwrap_or(false)
        || promotion
            .get("evidence_ids")
            .and_then(Value::as_array)
            .map(|values| values.is_empty() || values.iter().any(|value| value.as_str().is_none()))
            .unwrap_or(true)
        || promotion
            .get("artifact_hash")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        || promotion
            .get("scope_hash")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
        || promotion
            .get("public_api_hash")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_none()
    {
        fail("INVALID_PROMOTION_RECORD");
    }
    let gates = promotion
        .get("required_gate_results")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| fail("INVALID_PROMOTION_RECORD"));
    for gate in gates {
        for key in ["gate_id", "producer"] {
            if gate
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                fail("INVALID_PROMOTION_RECORD");
            }
        }
        if gate.get("result").and_then(Value::as_str).is_none() {
            fail("INVALID_PROMOTION_RECORD");
        }
    }
}

fn assert_record_graph(
    root: &Path,
    module_id: Option<&str>,
    artifact: &Value,
    require_freeze: bool,
) {
    let evidence_name = module_id
        .map(|id| module_record_name("evidence-record", id))
        .unwrap_or_else(|| "evidence-record.json".into());
    let review_name = module_id
        .map(|id| module_record_name("review-record", id))
        .unwrap_or_else(|| "review-record.json".into());
    let promotion_name = module_id
        .map(|id| module_record_name("promotion-record", id))
        .unwrap_or_else(|| "promotion-record.json".into());
    let evidence = read_record(root, &evidence_name);
    let review = read_record(root, &review_name);
    let promotion = read_record(root, &promotion_name);
    assert_record_schema(&evidence, &review, &promotion);
    let cleanup_id = record_str(
        &promotion,
        "/playground_cleanup_record_id",
        "promotion-record.json",
    );
    let cleanup = read_record(root, &format!("playground-cleanup-{}.json", cleanup_id));
    if record_str(&cleanup, "/cleanup_id", "playground-cleanup-record") != cleanup_id
        || !matches!(
            cleanup.get("disposition").and_then(Value::as_str),
            Some("archive_then_remove" | "remove" | "retain_open")
        )
    {
        fail("INVALID_PLAYGROUND_CLEANUP_RECORD");
    }
    let evidence_id = record_str(&evidence, "/evidence_id", "evidence-record.json");
    let issue_id = record_str(&evidence, "/issue_id", "evidence-record.json");
    let experiment_id = record_str(&evidence, "/experiment_id", "evidence-record.json");
    if record_str(&evidence, "/result", "evidence-record.json") != "pass"
        || record_str(&review, "/verdict", "review-record.json") != "pass"
    {
        fail("PROMOTION_EVIDENCE_NOT_PASSED");
    }
    if record_str(&review, "/issue_id", "review-record.json") != issue_id
        || record_str(&promotion, "/issue_id", "promotion-record.json") != issue_id
        || record_str(&promotion, "/experiment_id", "promotion-record.json") != experiment_id
    {
        fail("RECORD_GRAPH_SCOPE_MISMATCH");
    }
    if record_str(&review, "/promotion_id", "review-record.json")
        != record_str(&promotion, "/promotion_id", "promotion-record.json")
        || record_str(&review, "/review_id", "review-record.json")
            != record_str(&promotion, "/review_id", "promotion-record.json")
    {
        fail("RECORD_GRAPH_REFERENCE_MISMATCH");
    }
    let review_evidence_ids = record_array(&review, "/evidence_ids", "review-record.json");
    let promotion_evidence_ids = record_array(&promotion, "/evidence_ids", "promotion-record.json");
    if !review_evidence_ids
        .iter()
        .any(|id| id.as_str() == Some(evidence_id))
        || !promotion_evidence_ids
            .iter()
            .any(|id| id.as_str() == Some(evidence_id))
    {
        fail("RECORD_GRAPH_EVIDENCE_REFERENCE_MISMATCH");
    }
    if let Some(module_id) = module_id {
        if record_str(&promotion, "/module_id", "promotion-record.json") != module_id
            || evidence.pointer("/scope/module_id").and_then(Value::as_str) != Some(module_id)
        {
            fail("RECORD_GRAPH_MODULE_MISMATCH");
        }
    }
    let artifact_hash = record_str(artifact, "/artifact_hash", "artifact");
    if record_str(&review, "/reviewed_artifact_hash", "review-record.json") != artifact_hash
        || record_str(&promotion, "/artifact_hash", "promotion-record.json") != artifact_hash
    {
        fail("RECORD_GRAPH_ARTIFACT_MISMATCH");
    }
    if record_str(&review, "/reviewed_commit", "review-record.json")
        != record_str(&promotion, "/source_commit", "promotion-record.json")
        || record_str(&evidence, "/source_commit", "evidence-record.json")
            != record_str(&promotion, "/source_commit", "promotion-record.json")
        || record_str(&review, "/reviewed_scope_hash", "review-record.json")
            != record_str(&promotion, "/scope_hash", "promotion-record.json")
        || record_str(&evidence, "/scope_hash", "evidence-record.json")
            != record_str(&promotion, "/scope_hash", "promotion-record.json")
    {
        fail("RECORD_GRAPH_INPUT_MISMATCH");
    }
    let confidence = review
        .get("ai_confidence")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| fail("INVALID_REVIEW_CONFIDENCE"));
    if !(0.0..=1.0).contains(&confidence)
        || record_str(&review, "/confidence_rationale", "review-record.json").is_empty()
    {
        fail("INVALID_REVIEW_CONFIDENCE");
    }
    let gates = record_array(
        &promotion,
        "/required_gate_results",
        "promotion-record.json",
    );
    if gates
        .iter()
        .any(|gate| gate.get("result").and_then(Value::as_str) != Some("pass"))
    {
        fail("PROMOTION_GATE_NOT_PASSED");
    }
    let expires_at = record_str(&evidence, "/expires_at", "evidence-record.json");
    let expires_at = DateTime::parse_from_rfc3339(expires_at)
        .unwrap_or_else(|_| fail("INVALID_EVIDENCE_EXPIRY"))
        .with_timezone(&Utc);
    if expires_at <= Utc::now() {
        fail("EVIDENCE_EXPIRED");
    }
    if require_freeze {
        let module_id = module_id.unwrap_or_else(|| fail("FREEZE_RECORD_MODULE_REQUIRED"));
        let freeze_name = freeze_record_name(module_id);
        let freeze = read_record(root, &freeze_name);
        let active_root = contract_root(root, &read_project(root), "/governance/active_root");
        let active_version = record_str(&freeze, "/active_version", &freeze_name);
        let active_artifact = active_root
            .join(module_id)
            .join(active_version)
            .join("artifact.json");
        if active_artifact.is_file()
            && !fs::symlink_metadata(&active_artifact)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
        {
            let active_value: Value = serde_json::from_str(
                &fs::read_to_string(&active_artifact)
                    .unwrap_or_else(|_| fail("ACTIVE_ARTIFACT_MISSING")),
            )
            .unwrap_or_else(|_| fail("INVALID_ACTIVE_ARTIFACT"));
            if record_str(&active_value, "/artifact_hash", "active_artifact")
                != record_str(&freeze, "/library_hash", &freeze_name)
            {
                fail("FREEZE_ACTIVE_HASH_MISMATCH");
            }
        }
        for path in [
            "/freeze_id",
            "/issue_id",
            "/module_id",
            "/promotion_id",
            "/promotion_record_hash",
            "/artifact_record_id",
            "/source_commit_or_tag",
            "/active_version",
            "/library_hash",
            "/public_api_hash",
            "/review_id",
            "/created_at",
            "/clean_scope/base_commit",
            "/clean_scope/generated_policy",
        ] {
            record_str(&freeze, path, &freeze_name);
        }
        for path in ["/previous_active_immutable", "/git_clean"] {
            if freeze
                .get(path.trim_start_matches('/'))
                .and_then(Value::as_bool)
                .is_none()
            {
                fail(format!("INVALID_RECORD:{}:{}", freeze_name, path));
            }
        }
        let clean_scope = freeze
            .get("clean_scope")
            .and_then(Value::as_object)
            .unwrap_or_else(|| fail(format!("INVALID_RECORD:{}:/clean_scope", freeze_name)));
        for key in ["changed_paths", "ignored_paths"] {
            if clean_scope.get(key).and_then(Value::as_array).is_none() {
                fail(format!(
                    "INVALID_RECORD:{}/clean_scope/{}",
                    freeze_name, key
                ));
            }
        }
        let owners = freeze
            .get("owners")
            .and_then(Value::as_object)
            .unwrap_or_else(|| fail(format!("INVALID_RECORD:{}:/owners", freeze_name)));
        for key in [
            "vcs",
            "compiler",
            "api_extractor",
            "review",
            "artifact_registry",
        ] {
            if owners
                .get(key)
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .is_none()
            {
                fail(format!("INVALID_RECORD:{}:/owners/{}", freeze_name, key));
            }
        }
        if record_str(&freeze, "/module_id", &freeze_name) != module_id {
            fail("FREEZE_RECORD_MODULE_MISMATCH");
        }
        if record_str(&freeze, "/promotion_id", &freeze_name)
            != record_str(&promotion, "/promotion_id", "promotion-record.json")
        {
            fail("FREEZE_RECORD_PROMOTION_MISMATCH");
        }
        if record_str(&freeze, "/review_id", &freeze_name)
            != record_str(&review, "/review_id", "review-record.json")
        {
            fail("FREEZE_RECORD_REVIEW_MISMATCH");
        }
        if record_str(&freeze, "/library_hash", &freeze_name) != artifact_hash {
            fail("FREEZE_RECORD_LIBRARY_HASH_MISMATCH");
        }
        if record_str(&freeze, "/active_version", &freeze_name)
            != record_str(&promotion, "/new_active_version", "promotion-record.json")
        {
            fail("FREEZE_RECORD_VERSION_MISMATCH");
        }
        if record_str(&freeze, "/artifact_record_id", &freeze_name) != evidence_id {
            fail("FREEZE_RECORD_ARTIFACT_RECORD_MISMATCH");
        }
        if record_str(&freeze, "/promotion_record_hash", &freeze_name)
            != sha256(&canonical(&promotion))
        {
            fail("FREEZE_RECORD_PROMOTION_HASH_MISMATCH");
        }
        if record_str(&freeze, "/public_api_hash", &freeze_name)
            != record_str(&promotion, "/public_api_hash", "promotion-record.json")
        {
            fail("FREEZE_RECORD_PUBLIC_API_HASH_MISMATCH");
        }
        if record_str(&freeze, "/source_commit_or_tag", &freeze_name).is_empty()
            || record_str(&freeze, "/public_api_hash", &freeze_name).is_empty()
        {
            fail("FREEZE_RECORD_REQUIRED_FIELD_MISMATCH");
        }
        if record_str(&freeze, "/source_commit_or_tag", &freeze_name)
            != record_str(&promotion, "/source_commit", "promotion-record.json")
        {
            fail("FREEZE_RECORD_SOURCE_COMMIT_MISMATCH");
        }
        if freeze.get("git_clean") != Some(&Value::Bool(true)) {
            fail("FREEZE_REQUIREMENTS_NOT_MET");
        }
        let previous_version = freeze
            .get("previous_active_version")
            .and_then(Value::as_str);
        let previous_immutable = freeze
            .get("previous_active_immutable")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| fail("FREEZE_REQUIREMENTS_NOT_MET"));
        if previous_immutable != previous_version.is_some() {
            fail("FREEZE_PREVIOUS_ACTIVE_CLAIM_MISMATCH");
        }
        if let Some(previous) = freeze
            .get("previous_active_version")
            .and_then(Value::as_str)
        {
            let project = read_project(root);
            let active_root = contract_root(root, &project, "/governance/active_root");
            let previous_path = active_root.join(module_id).join(previous);
            assert_no_symlink_components(root, &previous_path, "previous_active");
            if !previous_path.is_dir() {
                fail("PREVIOUS_ACTIVE_MISSING");
            }
            let artifact = previous_path.join("artifact.json");
            if fs::symlink_metadata(&artifact)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                fail("PREVIOUS_ACTIVE_SYMLINK");
            }
            if !artifact.is_file() {
                fail("PREVIOUS_ACTIVE_ARTIFACT_MISSING");
            }
            let previous_value: Value = serde_json::from_str(
                &fs::read_to_string(&artifact)
                    .unwrap_or_else(|_| fail("PREVIOUS_ACTIVE_ARTIFACT_MISSING")),
            )
            .unwrap_or_else(|_| fail("INVALID_PREVIOUS_ACTIVE_ARTIFACT"));
            assert_artifact_matches(&project, &previous_value);
            let previous_hash = record_str(
                &previous_value,
                "/artifact_hash",
                "previous_active_artifact",
            );
            let promotion = read_record(root, &module_record_name("promotion-record", module_id));
            if promotion
                .pointer("/base_artifact_hash")
                .and_then(Value::as_str)
                != Some(previous_hash)
            {
                fail("PREVIOUS_ACTIVE_HASH_MISMATCH");
            }
            if previous_value
                .pointer("/modules")
                .and_then(Value::as_array)
                .map(|modules| {
                    modules.iter().any(|entry| {
                        entry.get("module_id").and_then(Value::as_str) == Some(module_id)
                    })
                })
                .unwrap_or(false)
                == false
            {
                fail("PREVIOUS_ACTIVE_MODULE_MISSING");
            }
        }
    }
}

fn promote(root: &Path, target: &str) {
    assert_project_root_safe(root);
    let project = read_project(root);
    assert_project_contract(root, &project);
    assert_goal_confirmed(root);
    assert_declared_contracts(root, &project, true);
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

fn promote_module(root: &Path, module_id: &str, target: &str) {
    assert_project_root_safe(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    if target == "frozen" && recover_freeze_transaction(root, &project, module_id) {
        println!("{}", serde_json::to_string_pretty(&project).unwrap());
        return;
    }
    assert_declared_contracts(root, &project, true);
    if target != "frozen"
        && project
            .get("modules")
            .and_then(Value::as_array)
            .map(|modules| {
                modules.iter().any(|module| {
                    module.get("module_id").and_then(Value::as_str) != Some(module_id)
                        && module.get("stage").and_then(Value::as_str) == Some("frozen")
                })
            })
            .unwrap_or(false)
    {
        fail("FROZEN_MODULE_REQUIRES_VERSIONED_ARTIFACT");
    }
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
    if target == "architecture_stable" {
        assert_record_graph(root, Some(module_id), &artifact, false);
    }
    write_artifact_value(root, &project, &artifact);
    write_project(root, &candidate);
    println!("{}", serde_json::to_string_pretty(&candidate).unwrap());
}

fn freeze_module(root: &Path, module_id: &str) {
    assert_project_root_safe(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    let project = read_project(root);
    if recover_freeze_transaction(root, &project, module_id) {
        println!("{}", serde_json::to_string_pretty(&project).unwrap());
        return;
    }
    assert_declared_contracts(root, &project, true);
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
    assert_compile_preconditions(root, &project, Some(module_id));
    let promoted_artifact = read_compiled_artifact(root, &project);
    assert_artifact_matches(&project, &promoted_artifact);
    assert_record_graph(root, Some(module_id), &promoted_artifact, false);
    let archive = contract_root(root, &project, "/governance/protected_root")
        .join("history")
        .join(module_id);
    let staging_archive = contract_root(root, &project, "/governance/protected_root")
        .join("history")
        .join(format!(".{}.staging.{}", module_id, std::process::id()));
    assert_no_symlink_components(root, &archive, "protected_archive");
    assert_no_symlink_components(root, &staging_archive, "protected_archive_staging");
    if archive.exists() {
        fail(format!("PROTECTED_HISTORY_IMMUTABLE:{}", module_id));
    }
    if staging_archive.exists() {
        fail(format!("PROTECTED_ARCHIVE_STAGING_EXISTS:{}", module_id));
    }
    let review_name = module_record_name("review-record", module_id);
    let promotion_name = module_record_name("promotion-record", module_id);
    let review = read_record(root, &review_name);
    let promotion = read_record(root, &promotion_name);
    let freeze_name = freeze_record_name(module_id);
    let mut freeze = read_record(root, &freeze_name);
    let artifact = promoted_artifact.clone();
    let reviewed_hash = record_str(&artifact, "/artifact_hash", "artifact");
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
    for path in candidate["modules"][index]
        .get("owned_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULE_SURFACES"))
    {
        let relative = path
            .as_str()
            .unwrap_or_else(|| fail("INVALID_MODULE_SURFACES"));
        if !safe_owned_path(root, relative, "owned_path").exists() {
            fail("PROTECTED_ARCHIVE_SOURCE_MISSING");
        }
    }
    fs::create_dir_all(&staging_archive).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
    fs::write(
        staging_archive.join("freeze-artifact.json"),
        serde_json::to_string_pretty(&artifact).unwrap() + "\n",
    )
    .unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
    fs::write(
        staging_archive.join("module-contract.json"),
        serde_json::to_string_pretty(&candidate["modules"][index]).unwrap() + "\n",
    )
    .unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
    for path in candidate["modules"][index]
        .get("owned_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULE_SURFACES"))
    {
        let relative = path
            .as_str()
            .unwrap_or_else(|| fail("INVALID_MODULE_SURFACES"));
        let source = safe_owned_path(root, relative, "owned_path");
        let target = staging_archive
            .join("source")
            .join(relative.trim_end_matches("/**").trim_end_matches('/'));
        if relative.ends_with("/**") {
            if !source.exists() {
                fail("PROTECTED_ARCHIVE_SOURCE_MISSING");
            }
            copy_tree(&source, &target);
        } else if source.exists() && source.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
            }
            fs::copy(&source, target).unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
        } else {
            fail("PROTECTED_ARCHIVE_SOURCE_MISSING");
        }
    }
    let mut source_snapshot = serde_json::Map::new();
    source_snapshot.insert("module_id".into(), Value::String(module_id.into()));
    source_snapshot.insert(
        "source_commit_or_tag".into(),
        Value::String(record_str(&freeze, "/source_commit_or_tag", &freeze_name).into()),
    );
    fs::write(
        staging_archive.join("source-snapshot.json"),
        serde_json::to_string_pretty(&Value::Object(source_snapshot)).unwrap() + "\n",
    )
    .unwrap_or_else(|_| fail("PROTECTED_ARCHIVE_FAILED"));
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
    write_artifact_value(root, &candidate, &artifact);
    write_record(root, &review_name, &review);
    write_record(root, &promotion_name, &promotion);
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

fn publish_active(root: &Path, module_id: &str, version: &str) {
    assert_project_root_safe(root);
    assert_identifier(module_id, "INVALID_MODULE_ID");
    if version.is_empty()
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        || version == "."
        || version == ".."
    {
        fail("INVALID_ACTIVE_VERSION");
    }
    let project = read_project(root);
    assert_project_contract(root, &project);
    assert_declared_contracts(root, &project, true);
    let stage = required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT");
    assert_sdk_lock(
        root,
        &project,
        matches!(
            stage,
            "compiled" | "controlled_verified" | "architecture_stable" | "frozen" | "retired"
        ),
    );
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
    let artifact_file = generated_root(root, &project).join("project.compiled.json");
    if fs::symlink_metadata(&artifact_file)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:artifact");
    }
    let artifact: Value = serde_json::from_str(
        &fs::read_to_string(&artifact_file)
            .unwrap_or_else(|_| fail("COMPILED_STAGE_REQUIRES_ARTIFACT")),
    )
    .unwrap_or_else(|_| fail("INVALID_ARTIFACT_SCHEMA"));
    assert_artifact_matches(&project, &artifact);
    assert_record_graph(root, Some(module_id), &artifact, true);
    let artifact_hash = record_str(&artifact, "/artifact_hash", "artifact");
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
    println!("active {} {}", module_id, version);
}

fn verify(root: &Path) {
    assert_project_root_safe(root);
    let project = read_project(root);
    assert_governance_maps(root);
    assert_declared_contracts(root, &project, true);
    assert_project_contract(root, &project);
    if project.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail("UNSUPPORTED_PROJECT_SCHEMA");
    }
    if required_str(&project, "/sdk/name", "INVALID_SDK_CONTRACT") != "appsdk" {
        fail("INVALID_SDK_CONTRACT");
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
        {
            fail("INVALID_MODULE_SURFACES");
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
    assert_sdk_lock(
        root,
        &project,
        matches!(
            stage,
            "compiled" | "controlled_verified" | "architecture_stable" | "frozen" | "retired"
        ),
    );
    assert_goal_contract(root, false);
    let lock_file = root.join(".appsdk").join("sdk.lock");
    let lock: Value = serde_json::from_str(
        &fs::read_to_string(&lock_file).unwrap_or_else(|_| fail("MISSING_SDK_LOCK")),
    )
    .unwrap_or_else(|_| fail("INVALID_SDK_LOCK"));
    if lock.get("sdk").and_then(Value::as_str) != Some("appsdk")
        || lock.get("version").and_then(Value::as_str)
            != project.pointer("/sdk/version").and_then(Value::as_str)
    {
        fail("INVALID_SDK_LOCK");
    }
    let artifact_file = generated_root(root, &project).join("project.compiled.json");
    let stage = required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT");
    if matches!(
        stage,
        "compiled" | "controlled_verified" | "architecture_stable" | "frozen" | "retired"
    ) && !artifact_file.exists()
    {
        fail("COMPILED_STAGE_REQUIRES_ARTIFACT");
    }
    if artifact_file.exists() {
        let artifact = read_compiled_artifact(root, &project);
        assert_artifact_matches(&project, &artifact);
    }
    let artifact = if artifact_file.exists() {
        Some(read_compiled_artifact(root, &project))
    } else {
        None
    };
    for module in project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"))
    {
        if module.get("stage").and_then(Value::as_str) == Some("frozen") {
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
                assert_artifact_matches(&project, &active_value);
                if let Some(generated) = &artifact {
                    if record_str(generated, "/artifact_hash", "artifact")
                        != record_str(&active_value, "/artifact_hash", "active_artifact")
                    {
                        fail("ACTIVE_ARTIFACT_HASH_MISMATCH");
                    }
                }
            }
        }
    }
    if project
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
        let artifact = read_compiled_artifact(root, &project);
        for module in project
            .get("modules")
            .and_then(Value::as_array)
            .unwrap_or_else(|| fail("INVALID_MODULES_CONTRACT"))
        {
            if matches!(
                module.get("stage").and_then(Value::as_str),
                Some("architecture_stable" | "frozen" | "retired")
            ) {
                assert_record_graph(
                    root,
                    module.get("module_id").and_then(Value::as_str),
                    &artifact,
                    matches!(
                        module.get("stage").and_then(Value::as_str),
                        Some("frozen" | "retired")
                    ),
                );
            }
        }
    }
    println!(
        "{{\"ok\":true,\"project_id\":\"{}\",\"stage\":\"{}\"}}",
        required_str(&project, "/project_id", "INVALID_PROJECT_ID"),
        required_str(&project, "/lifecycle/stage", "INVALID_LIFECYCLE_CONTRACT")
    );
}

const APPSDK_GITIGNORE_BEGIN: &str = "# BEGIN APPSDK MANAGED";
const APPSDK_GITIGNORE_END: &str = "# END APPSDK MANAGED";
const APPSDK_GITIGNORE_BLOCK: &str =
    "# BEGIN APPSDK MANAGED\n.appsdk-control/\n.appsdk/sdk.bin\n/active/lib/\n/generated/\n# END APPSDK MANAGED\n";

fn ensure_appsdk_gitignore(root: &Path) {
    let path = root.join(".gitignore");
    if fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:gitignore");
    }
    let mut content = fs::read_to_string(&path).unwrap_or_default();
    if let Some(begin) = content.find(APPSDK_GITIGNORE_BEGIN) {
        let end_start = begin + APPSDK_GITIGNORE_BEGIN.len();
        let end = content[end_start..]
            .find(APPSDK_GITIGNORE_END)
            .map(|offset| end_start + offset)
            .unwrap_or_else(|| fail("INVALID_APPSDK_GITIGNORE_BLOCK"));
        let end_after = end + APPSDK_GITIGNORE_END.len();
        let mut updated = String::with_capacity(content.len());
        updated.push_str(&content[..begin]);
        updated.push_str(APPSDK_GITIGNORE_BLOCK);
        let suffix = &content[end_after..];
        if !suffix.trim().is_empty() {
            updated.push_str(suffix);
        }
        if updated != content {
            fs::write(path, updated).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
        }
        return;
    }
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.is_empty() {
        content.push('\n');
    }
    content.push_str(APPSDK_GITIGNORE_BLOCK);
    fs::write(path, content).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
}

fn ensure_governance_layout(root: &Path) {
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

fn write_if_missing(root: &Path, relative: &str, content: &str) {
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

fn write_project_scaffold(root: &Path) {
    write_if_missing(
        root,
        ".appsdk/project.json",
        r#"{
  "schema_version": 1,
  "project_id": "change-me",
  "sdk": {"name": "appsdk", "version": "0.1.0"},
  "lifecycle": {"stage": "draft"},
  "access": {"protected_paths": [".appsdk/**", "generated/**", "protected/source/**"]},
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
    "record_contracts": ["contracts/records/evidence-record.schema.json", "contracts/records/goal-clarification-record.schema.json", "contracts/records/review-record.schema.json", "contracts/records/promotion-record.schema.json", "contracts/records/freeze-record.schema.json", "contracts/records/record-graph.contract.json"],
    "zone_transition_contract": "contracts/transitions/zone-transition-manifest.json",
    "playground_retention": "archive_then_remove",
    "debug_merge_comment_required": true
  },
  "lifecycles": {"issue": "open", "library": "draft", "source_snapshot": "mutable", "artifact": "generated"},
  "modules": [{"module_id":"app-core","stage":"source_implemented","owned_paths":["playground/experiments/**","protected/source/**","tests/core/**"],"source_owner":"app-core","active_artifact":"active/lib/app-core/**","generated_outputs":["generated/**"]}]
}
"#,
    );
    write_if_missing(
        root,
        ".appsdk/goal.json",
        r#"{"goal_id":"goal-change-me","raw_request":"Describe the intended change before implementation.","understood_objective":"The objective will be restated and confirmed before admission.","acceptance_criteria":["The user-confirmed acceptance criteria are recorded before implementation."],"non_goals":[],"assumptions":[],"ambiguities":[],"questions":[],"status":"received","confirmed_by":null,"confirmed_at":null,"created_at":"2026-01-01T00:00:00Z"}
"#,
    );
    write_if_missing(
        root,
        ".appsdk/sdk.lock",
        r#"{"sdk":"appsdk","version":"0.1.0","digest":"sha256:replace-with-compiled-sdk-digest","compiler_digest":"sha256:replace-with-compiler-digest","contract_schema":1}
"#,
    );
}

fn init_project(root: &Path) {
    if root.exists()
        && fs::symlink_metadata(root)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    {
        fail(format!("TARGET_SYMLINK:{}", root.display()));
    }
    fs::create_dir_all(root).unwrap_or_else(|_| fail("PROJECT_CREATE_FAILED"));
    ensure_governance_layout(root);
    write_project_scaffold(root);
    println!("initialized {}", root.display());
}

fn new_project(root: &Path) {
    if root.exists() {
        if fs::symlink_metadata(root)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(format!("TARGET_SYMLINK:{}", root.display()));
        }
        if fs::read_dir(root)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(true)
        {
            fail(format!("TARGET_NOT_EMPTY:{}", root.display()));
        }
    }
    let mut parent = root.parent().unwrap_or(root).to_path_buf();
    while !parent.exists() {
        let next = parent.parent().unwrap_or(&parent).to_path_buf();
        if next == parent {
            break;
        }
        parent = next;
    }
    for ancestor in parent.ancestors() {
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
    ensure_governance_layout(root);
    write_project_scaffold(root);
    println!("created {}", root.display());
}

fn pin_lock(root: &Path, binary: &Path) {
    assert_project_root_safe(root);
    assert_no_symlink_components(root, &root.join(".appsdk"), "appsdk_control");
    let project = read_project(root);
    let binary = binary
        .canonicalize()
        .unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
    let bytes = fs::read(&binary).unwrap_or_else(|_| fail("SDK_BINARY_MISSING"));
    let digest = digest_bytes(&bytes);
    let mut lock = serde_json::Map::new();
    lock.insert("sdk".into(), Value::String("appsdk".into()));
    lock.insert(
        "version".into(),
        Value::String(required_str(&project, "/sdk/version", "INVALID_SDK_CONTRACT").into()),
    );
    lock.insert("digest".into(), Value::String(digest.clone()));
    lock.insert("compiler_digest".into(), Value::String(digest));
    let pinned_binary = root.join(".appsdk/sdk.bin");
    if fs::symlink_metadata(&pinned_binary)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        fail("GOVERNANCE_PATH_SYMLINK:sdk_binary");
    }
    fs::copy(&binary, &pinned_binary).unwrap_or_else(|_| fail("SDK_BINARY_WRITE_FAILED"));
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
    fs::write(
        lock_path,
        serde_json::to_string_pretty(&Value::Object(lock)).unwrap() + "\n",
    )
    .unwrap_or_else(|_| fail("SDK_LOCK_WRITE_FAILED"));
    println!("pinned {}", binary.display());
}

fn main() {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("version") => println!("appsdk 0.1.0 (rust)"),
        Some("verify") => verify(Path::new(&args.next().unwrap_or_else(|| ".".into()))),
        Some("pin-lock") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk pin-lock <dir> --binary <path>")));
            if args.next().as_deref() != Some("--binary") { fail("USAGE: appsdk pin-lock <dir> --binary <path>"); }
            let binary = args.next().unwrap_or_else(|| fail("USAGE: appsdk pin-lock <dir> --binary <path>"));
            pin_lock(&root, Path::new(&binary));
        }
        Some("compile") => compile(Path::new(&args.next().unwrap_or_else(|| ".".into()))),
        Some("promote") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk promote <dir> --to <stage>")));
            if args.next().as_deref() != Some("--to") { fail("USAGE: appsdk promote <dir> --to <stage>"); }
            promote(&root, &args.next().unwrap_or_else(|| fail("USAGE: appsdk promote <dir> --to <stage>")));
        }
        Some("promote-module") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk promote-module <dir> --module <id> --to <stage>")));
            if args.next().as_deref() != Some("--module") { fail("USAGE: appsdk promote-module <dir> --module <id> --to <stage>"); }
            let module_id = args.next().unwrap_or_else(|| fail("USAGE: appsdk promote-module <dir> --module <id> --to <stage>"));
            if args.next().as_deref() != Some("--to") { fail("USAGE: appsdk promote-module <dir> --module <id> --to <stage>"); }
            let target = args.next().unwrap_or_else(|| fail("USAGE: appsdk promote-module <dir> --module <id> --to <stage>"));
            promote_module(&root, &module_id, &target);
        }
        Some("freeze") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk freeze <dir> --module <id>")));
            if args.next().as_deref() != Some("--module") { fail("USAGE: appsdk freeze <dir> --module <id>"); }
            freeze_module(&root, &args.next().unwrap_or_else(|| fail("USAGE: appsdk freeze <dir> --module <id>")));
        }
        Some("publish-active") => {
            let root = PathBuf::from(args.next().unwrap_or_else(|| fail("USAGE: appsdk publish-active <dir> --module <id> --version <version>")));
            if args.next().as_deref() != Some("--module") { fail("USAGE: appsdk publish-active <dir> --module <id> --version <version>"); }
            let module_id = args.next().unwrap_or_else(|| fail("USAGE: appsdk publish-active <dir> --module <id> --version <version>"));
            if args.next().as_deref() != Some("--version") { fail("USAGE: appsdk publish-active <dir> --module <id> --version <version>"); }
            let version = args.next().unwrap_or_else(|| fail("USAGE: appsdk publish-active <dir> --module <id> --version <version>"));
            publish_active(&root, &module_id, &version);
        }
        Some("new") => {
            let root = args.next().unwrap_or_else(|| fail("USAGE: appsdk new <dir>"));
            if args.next().is_some() {
                fail("USAGE: appsdk new <dir>");
            }
            new_project(Path::new(&root));
        }
        Some("init") => {
            let root = args.next().unwrap_or_else(|| fail("USAGE: appsdk init <dir>"));
            if args.next().is_some() {
                fail("USAGE: appsdk init <dir>");
            }
            init_project(Path::new(&root));
        }
        _ => fail("USAGE: appsdk version | init <dir> | new <dir> | verify <dir> | pin-lock <dir> --binary <path> | compile <dir> | promote <dir> --to <stage> | promote-module <dir> --module <id> --to <stage> | freeze <dir> --module <id> | publish-active <dir> --module <id> --version <version>"),
    }
}

#[allow(dead_code)]
fn digest_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}
