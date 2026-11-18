use super::*;

pub(super) fn project_goal_apply(root: &Path, input_path: &str, format_json: bool) {
    assert_project_root_safe(root);
    let input_file = producer_input_path(root, input_path);
    let request: Value = serde_json::from_str(
        &fs::read_to_string(input_file).unwrap_or_else(|_| fail("GOAL_APPLY_INPUT_READ_FAILED")),
    )
    .unwrap_or_else(|_| fail("GOAL_APPLY_INPUT_INVALID"));
    if !request.is_object() {
        fail("GOAL_APPLY_INPUT_INVALID");
    }

    let project = read_project(root);
    let project_id = project
        .get("project_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail("GOAL_APPLY_PROJECT_ID_INVALID"));
    validate_goal_apply_request(&request, project_id);
    validate_goal_apply_scope(&request, &project);

    let operation = request
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("GOAL_APPLY_OPERATION_INVALID"));
    let requested_requirements_version = request
        .get("requirements_version")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fail("GOAL_APPLY_REQUIREMENTS_VERSION_INVALID"));
    let _lock = reset_transaction_acquire_lock(root).unwrap_or_else(|error| fail(error));

    let current_requirements_version = match requirements::read_requirements_if_present(root) {
        Some(ledger) => ledger
            .get("version")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| fail("REQUIREMENTS_INVALID")),
        None => 0,
    };
    if requested_requirements_version != current_requirements_version {
        if current_requirements_version == 0 && requested_requirements_version != 0 {
            fail("REQUIREMENTS_MISSING");
        }
        fail("REQUIREMENTS_VERSION_STALE");
    }

    let goal_path = root.join(".appsdk/goal.json");
    assert_no_symlink_components(root, &goal_path, "goal");
    let previous = read_goal_for_apply(root);
    let current_revision = previous.as_ref().map(goal_revision).unwrap_or(0);
    let base_revision = request.get("base_goal_revision").and_then(|value| {
        if value.is_null() {
            None
        } else {
            value.as_u64()
        }
    });
    match operation {
        "create" => {
            if previous.is_some() {
                fail("GOAL_APPLY_ALREADY_EXISTS");
            }
            if request.get("base_goal_revision").is_some()
                && !request["base_goal_revision"].is_null()
            {
                fail("GOAL_APPLY_BASE_INVALID");
            }
        }
        "replace" => {
            if previous.is_none() {
                fail("GOAL_APPLY_CURRENT_MISSING");
            }
            if base_revision != Some(current_revision) {
                fail("GOAL_APPLY_BASE_CONFLICT");
            }
        }
        _ => fail("GOAL_APPLY_OPERATION_INVALID"),
    }

    let mut next_goal = request
        .get("goal")
        .cloned()
        .unwrap_or_else(|| fail("GOAL_APPLY_GOAL_MISSING"));
    validate_goal_apply_content(&next_goal);
    let next_revision = if previous.is_some() {
        current_revision.saturating_add(1)
    } else {
        1
    };
    let history = previous
        .as_ref()
        .map(|old| {
            let mut entries = old
                .get("replacement_history")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut preserved = old.clone();
            if let Some(object) = preserved.as_object_mut() {
                object.remove("replacement_history");
            }
            entries.push(serde_json::json!({
                "goal": preserved,
                "superseded_by_revision": next_revision,
                "status": "superseded"
            }));
            Value::Array(entries)
        })
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let object = next_goal
        .as_object_mut()
        .unwrap_or_else(|| fail("GOAL_APPLY_GOAL_INVALID"));
    object.insert("project_id".into(), Value::String(project_id.to_owned()));
    object.insert(
        "requirements_version".into(),
        Value::from(requested_requirements_version),
    );
    object.insert("goal_revision".into(), Value::from(next_revision));
    object.insert("replacement_history".into(), history);
    atomic_write_json(&goal_path, &next_goal, "GOAL_APPLY_WRITE_FAILED");

    let result = serde_json::json!({
        "ok": true,
        "operation": operation,
        "project_id": project_id,
        "goal_id": next_goal.get("goal_id"),
        "goal_revision": next_revision,
        "requirements_version": requested_requirements_version,
        "path": ".appsdk/goal.json",
        "admitted": false,
        "subscription_changed": false
    });
    if format_json {
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
    } else {
        println!(
            "Project goal applied: {} revision {}",
            next_goal["goal_id"].as_str().unwrap_or(""),
            next_revision
        );
    }
}

fn validate_goal_apply_request(request: &Value, project_id: &str) {
    let object = request
        .as_object()
        .unwrap_or_else(|| fail("GOAL_APPLY_INPUT_INVALID"));
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "operation"
                | "project_id"
                | "base_goal_revision"
                | "requirements_version"
                | "authorization"
                | "goal"
        ) {
            fail(format!("GOAL_APPLY_UNKNOWN_FIELD:{key}"));
        }
    }
    if request.get("project_id").and_then(Value::as_str) != Some(project_id) {
        fail("GOAL_APPLY_PROJECT_MISMATCH");
    }
    if !matches!(
        request.get("operation").and_then(Value::as_str),
        Some("create" | "replace")
    ) {
        fail("GOAL_APPLY_OPERATION_INVALID");
    }
    if request
        .get("requirements_version")
        .and_then(Value::as_u64)
        .is_none()
    {
        fail("GOAL_APPLY_REQUIREMENTS_VERSION_INVALID");
    }
    if request.get("base_goal_revision").is_none()
        || (!request["base_goal_revision"].is_null()
            && request["base_goal_revision"].as_u64().is_none())
    {
        fail("GOAL_APPLY_BASE_INVALID");
    }
    if !authorization_valid_for_goal_apply(request.get("authorization")) {
        fail("GOAL_APPLY_AUTHORIZATION_INVALID");
    }
    if !request.get("goal").is_some_and(Value::is_object) {
        fail("GOAL_APPLY_GOAL_INVALID");
    }
}

fn authorization_valid_for_goal_apply(authorization: Option<&Value>) -> bool {
    authorization
        .and_then(Value::as_object)
        .map(|object| {
            object.len() == 4
                && object.get("role").and_then(Value::as_str) == Some("user")
                && object
                    .get("source")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
                && object
                    .get("original_text")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
        })
        .unwrap_or(false)
}

fn validate_goal_apply_scope(request: &Value, project: &Value) {
    let authorized_scope = request
        .pointer("/authorization/scope")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("GOAL_APPLY_SCOPE_INVALID"));
    if authorized_scope.len() != 2 {
        fail("GOAL_APPLY_SCOPE_INVALID");
    }
    let authorized_allowed = authorized_scope
        .get("allowed_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("GOAL_APPLY_SCOPE_INVALID"));
    let authorized_forbidden = authorized_scope
        .get("forbidden_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("GOAL_APPLY_SCOPE_INVALID"));
    for value in authorized_allowed.iter().chain(authorized_forbidden) {
        validate_goal_scope_path(value);
    }

    let scope = request
        .pointer("/goal/scope")
        .and_then(Value::as_object)
        .unwrap_or_else(|| fail("GOAL_APPLY_SCOPE_INVALID"));
    for key in scope.keys() {
        if !matches!(
            key.as_str(),
            "feature_id" | "module_id" | "resource_ids" | "allowed_paths" | "forbidden_paths"
        ) {
            fail(format!("GOAL_APPLY_SCOPE_UNKNOWN_FIELD:{key}"));
        }
    }
    let allowed_paths = scope
        .get("allowed_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("GOAL_APPLY_SCOPE_INVALID"));
    let forbidden_paths = scope
        .get("forbidden_paths")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("GOAL_APPLY_SCOPE_INVALID"));
    for value in allowed_paths.iter().chain(forbidden_paths) {
        validate_goal_scope_path(value);
    }

    for value in allowed_paths {
        let path = value.as_str().unwrap();
        if authorized_forbidden
            .iter()
            .any(|item| path_is_covered_by(path, item.as_str().unwrap()))
            || !authorized_allowed
                .iter()
                .any(|item| path_is_covered_by(path, item.as_str().unwrap()))
        {
            fail("GOAL_APPLY_SCOPE_OUT_OF_AUTHORIZATION");
        }
    }
    for value in forbidden_paths {
        let path = value.as_str().unwrap();
        let covered = authorized_allowed
            .iter()
            .chain(authorized_forbidden)
            .any(|item| path_is_covered_by(path, item.as_str().unwrap()));
        if !covered {
            fail("GOAL_APPLY_SCOPE_OUT_OF_AUTHORIZATION");
        }
    }

    if let Some(module_id) = scope
        .get("module_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        let exists = project
            .get("modules")
            .and_then(Value::as_array)
            .is_some_and(|modules| {
                modules.iter().any(|module| {
                    module.get("module_id").and_then(Value::as_str) == Some(module_id)
                })
            });
        if !exists {
            fail("GOAL_APPLY_SCOPE_MODULE_UNKNOWN");
        }
    }
    if let Some(resource_ids) = scope.get("resource_ids").and_then(Value::as_array) {
        for resource_id in resource_ids {
            let id = resource_id
                .as_str()
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| fail("GOAL_APPLY_SCOPE_RESOURCE_UNKNOWN"));
            let exists = project
                .get("modules")
                .and_then(Value::as_array)
                .is_some_and(|modules| {
                    modules.iter().any(|module| {
                        module
                            .get("resource_ids")
                            .and_then(Value::as_array)
                            .is_some_and(|ids| ids.iter().any(|value| value.as_str() == Some(id)))
                    })
                });
            if !exists {
                fail("GOAL_APPLY_SCOPE_RESOURCE_UNKNOWN");
            }
        }
    } else if scope.contains_key("resource_ids") {
        fail("GOAL_APPLY_SCOPE_INVALID");
    }
}

fn validate_goal_scope_path(value: &Value) {
    let path = value
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fail("GOAL_APPLY_SCOPE_PATH_INVALID"));
    if path.starts_with('/')
        || path.contains('\\')
        || path.contains("//")
        || path.ends_with('/')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        fail("GOAL_APPLY_SCOPE_PATH_INVALID");
    }
    for (index, part) in path.split('/').enumerate() {
        if part.contains('*')
            && !(part == "**" || (part == "*" && index + 1 == path.split('/').count()))
        {
            fail("GOAL_APPLY_SCOPE_PATTERN_INVALID");
        }
    }
    if path.matches("**").count() > 1 || path.contains("***") {
        fail("GOAL_APPLY_SCOPE_PATTERN_INVALID");
    }
}

fn path_is_covered_by(path: &str, allowed: &str) -> bool {
    if allowed == "**" {
        return true;
    }
    if let Some(prefix) = allowed.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }
    allowed == path
}

fn validate_goal_apply_content(goal: &Value) {
    let object = goal
        .as_object()
        .unwrap_or_else(|| fail("GOAL_APPLY_GOAL_INVALID"));
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "goal_id"
                | "issue_id"
                | "raw_request"
                | "understood_objective"
                | "acceptance_criteria"
                | "non_goals"
                | "assumptions"
                | "ambiguities"
                | "questions"
                | "scope"
                | "risk_decisions"
                | "status"
                | "confirmed_by"
                | "confirmed_at"
                | "created_at"
        ) {
            fail(format!("GOAL_APPLY_GOAL_UNKNOWN_FIELD:{key}"));
        }
    }
    for forbidden in [
        "requirements_version",
        "project_id",
        "goal_revision",
        "replacement_history",
    ] {
        if object.contains_key(forbidden) {
            fail(format!("GOAL_APPLY_SERVER_FIELD:{forbidden}"));
        }
    }
    let status = goal
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("GOAL_APPLY_STATUS_INVALID"));
    if status == "admitted" {
        fail("GOAL_APPLY_ADMITTED_FORBIDDEN");
    }
    if !matches!(status, "clarification_pending" | "confirmed") {
        fail("GOAL_APPLY_STATUS_INVALID");
    }
    validate_goal_contract(goal, false);
    if goal
        .get("goal_id")
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
        || goal
            .get("raw_request")
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        || goal
            .get("understood_objective")
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        || goal
            .get("created_at")
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
    {
        fail("GOAL_APPLY_GOAL_INVALID");
    }
    let acceptance = goal
        .get("acceptance_criteria")
        .and_then(Value::as_array)
        .filter(|items| {
            !items.is_empty()
                && items
                    .iter()
                    .all(|item| item.as_str().is_some_and(|value| !value.trim().is_empty()))
        })
        .is_some();
    if !acceptance {
        fail("GOAL_APPLY_ACCEPTANCE_INVALID");
    }
    for key in ["non_goals", "assumptions", "ambiguities", "questions"] {
        if goal.get(key).and_then(Value::as_array).is_none() {
            let field = key.to_uppercase();
            fail(format!("GOAL_APPLY_{field}_INVALID"));
        }
    }
    if status == "confirmed" {
        let questions_open = goal["questions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|question| question.get("status").and_then(Value::as_str) == Some("open"));
        if questions_open {
            fail("GOAL_APPLY_OPEN_QUESTIONS");
        }
        if goal
            .get("confirmed_by")
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
            || goal
                .get("confirmed_at")
                .and_then(Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
        {
            fail("GOAL_APPLY_CONFIRMATION_INVALID");
        }
    }
}

fn read_goal_for_apply(root: &Path) -> Option<Value> {
    let file = root.join(".appsdk/goal.json");
    let metadata = match fs::symlink_metadata(&file) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(_) => fail("GOAL_APPLY_CURRENT_UNAVAILABLE"),
    };
    if metadata.file_type().is_symlink() {
        fail("GOVERNANCE_PATH_SYMLINK:goal");
    }
    let text = fs::read_to_string(file).unwrap_or_else(|_| fail("GOAL_APPLY_CURRENT_UNAVAILABLE"));
    let goal: Value =
        serde_json::from_str(&text).unwrap_or_else(|_| fail("GOAL_APPLY_CURRENT_INVALID"));
    validate_goal_contract(&goal, false);
    Some(goal)
}

fn goal_revision(goal: &Value) -> u64 {
    goal.get("goal_revision")
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

pub(super) fn assert_governance_maps(root: &Path) {
    try_assert_governance_maps(root).unwrap_or_else(|error| fail(error))
}

pub(super) fn try_assert_governance_maps(root: &Path) -> Result<(), String> {
    for (name, key) in [
        ("resource-map.json", "resources"),
        ("module-registry.json", "modules"),
        ("function-map.json", "functions"),
        ("mainline-call-map.json", "edges"),
        ("verification-map.json", "gates"),
    ] {
        let file = root.join(".appsdk/maps").join(name);
        let text =
            fs::read_to_string(&file).map_err(|_| format!("MISSING_GOVERNANCE_MAP:{}", name))?;
        let value: Value =
            serde_json::from_str(&text).map_err(|_| format!("INVALID_GOVERNANCE_MAP:{}", name))?;
        // Governance maps are project-owned projections. The SDK bundle
        // supplies schema/validation rules, but must not require byte-for-byte
        // equality with a generic SDK map; project modules may add or evolve
        // entries while retaining the same machine-readable contract.
        if value.get("schema_version").and_then(Value::as_u64) != Some(1)
            || value
                .get(key)
                .and_then(Value::as_array)
                .map(|items| items.is_empty())
                .unwrap_or(true)
        {
            return Err(format!("INVALID_GOVERNANCE_MAP:{}", name));
        }
        if name == "mainline-call-map.json" {
            for edge in try_record_array(&value, "/edges", name)? {
                for field in [
                    "/chain_id",
                    "/owner",
                    "/caller",
                    "/callee",
                    "/path",
                    "/input_resource_id",
                    "/output_resource_id",
                    "/error_resource_id",
                ] {
                    if try_record_str(edge, field, name)?.is_empty() {
                        return Err("UNBOUND_MAINLINE_EDGE".to_owned());
                    }
                }
            }
        }
    }
    Ok(())
}

pub(super) fn assert_registry_binding_contract<'a>(
    module: &'a Value,
    module_id: &str,
) -> RegistryBinding<'a> {
    try_assert_registry_binding_contract(module, module_id).unwrap_or_else(|error| fail(error))
}

pub(super) fn try_assert_registry_binding_contract<'a>(
    module: &'a Value,
    module_id: &str,
) -> Result<RegistryBinding<'a>, String> {
    let Some(binding) = module.get("registry_binding") else {
        return Ok(RegistryBinding::Exact);
    };
    let object = binding
        .as_object()
        .ok_or_else(|| format!("INVALID_REGISTRY_BINDING:{}", module_id))?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "mode" | "modules"))
    {
        return Err(format!("INVALID_REGISTRY_BINDING:{}", module_id));
    }
    let mode = object
        .get("mode")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("INVALID_REGISTRY_BINDING:{}", module_id))?;
    match mode {
        "exact" => {
            if object.contains_key("modules") {
                return Err(format!("INVALID_REGISTRY_BINDING:{}", module_id));
            }
            Ok(RegistryBinding::Exact)
        }
        "aggregate" => {
            let modules = object
                .get("modules")
                .and_then(Value::as_array)
                .filter(|modules| !modules.is_empty())
                .ok_or_else(|| format!("INVALID_REGISTRY_BINDING:{}", module_id))?;
            let mut ids = std::collections::HashSet::new();
            for selected in modules {
                let selected = selected
                    .as_str()
                    .filter(|selected| !selected.is_empty())
                    .ok_or_else(|| format!("INVALID_REGISTRY_BINDING:{}", module_id))?;
                if !ids.insert(selected) {
                    return Err(format!("INVALID_REGISTRY_BINDING:{}", module_id));
                }
            }
            Ok(RegistryBinding::Aggregate(modules))
        }
        _ => Err(format!("INVALID_REGISTRY_BINDING:{}", module_id)),
    }
}

pub(super) fn normalized_registry_binding(module: &Value, module_id: &str) -> Value {
    match assert_registry_binding_contract(module, module_id) {
        RegistryBinding::Exact => serde_json::json!({"mode": "exact"}),
        RegistryBinding::Aggregate(modules) => {
            let mut module_ids = modules
                .iter()
                .map(|module| {
                    module
                        .as_str()
                        .unwrap_or_else(|| fail(format!("INVALID_REGISTRY_BINDING:{}", module_id)))
                        .to_string()
                })
                .collect::<Vec<_>>();
            module_ids.sort();
            serde_json::json!({"mode": "aggregate", "modules": module_ids})
        }
    }
}

pub(super) fn assert_lifecycle_producer_map_binding(
    root: &Path,
    project: &Value,
    module_id: &str,
    producer: LifecycleProducer,
) {
    for name in GOVERNANCE_MAP_NAMES {
        assert_no_symlink_components(
            root,
            &root.join(".appsdk/maps").join(name),
            "lifecycle_producer_map",
        );
    }
    assert_no_symlink_components(
        root,
        &root.join(".appsdk/maps/module-registry.json"),
        "lifecycle_producer_module_registry",
    );
    assert_governance_maps(root);
    // Governance maps are project-owned projections. A project may omit the
    // SDK producer projection when it does not publish that control-plane
    // entry; if a producer-owned identity is present, its declaration must
    // remain canonical and unique.
    let mut maps = std::collections::HashMap::new();
    for name in GOVERNANCE_MAP_NAMES {
        let path = root.join(".appsdk/maps").join(name);
        assert_no_symlink_components(root, &path, "lifecycle_producer_map");
        let value: Value = serde_json::from_str(
            &fs::read_to_string(&path)
                .unwrap_or_else(|_| fail(format!("LIFECYCLE_PRODUCER_MAP_MISSING:{}", name))),
        )
        .unwrap_or_else(|_| fail(format!("LIFECYCLE_PRODUCER_MAP_INVALID:{}", name)));
        let key = match name {
            "resource-map.json" => "resources",
            "function-map.json" => "functions",
            "mainline-call-map.json" => "edges",
            "verification-map.json" => "gates",
            _ => unreachable!(),
        };
        if value.get("schema_version").and_then(Value::as_u64) != Some(1)
            || value
                .get(key)
                .and_then(Value::as_array)
                .is_none_or(|items| items.is_empty())
        {
            fail(format!("LIFECYCLE_PRODUCER_MAP_INVALID:{}", name));
        }
        maps.insert(name, value);
    }
    for (name, key) in [
        ("resource-map.json", "resources"),
        ("function-map.json", "functions"),
        ("mainline-call-map.json", "edges"),
        ("verification-map.json", "gates"),
    ] {
        let actual = maps
            .get(name)
            .unwrap()
            .get(key)
            .unwrap()
            .as_array()
            .unwrap();
        let canonical: Value = serde_json::from_str(canonical_governance_map(name))
            .unwrap_or_else(|_| fail(format!("LIFECYCLE_PRODUCER_MAP_INVALID:{}", name)));
        let canonical_entries: Vec<&Value> = canonical
            .get(key)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .collect();
        let entry_id = |entry: &Value| -> Option<String> {
            match name {
                "resource-map.json" => entry
                    .get("resource_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                "function-map.json" => entry
                    .get("function_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                "mainline-call-map.json" => entry
                    .get("chain_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                "verification-map.json" => entry
                    .get("gate_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                _ => None,
            }
        };
        let required_entries: Vec<&Value> = canonical_entries
            .iter()
            .copied()
            .filter(|entry| {
                let id = entry_id(entry);
                match (producer, name) {
                    (LifecycleProducer::Records, "resource-map.json") => matches!(
                        id.as_deref(),
                        Some("lifecycle_record_producer_input")
                            | Some("fix_worktree")
                            | Some("fix_evidence_set")
                            | Some("fix_reproduction")
                    ),
                    (LifecycleProducer::Chain, "resource-map.json") => {
                        id.as_deref() == Some("lifecycle_chain_producer_input")
                    }
                    (LifecycleProducer::Records, "function-map.json") => {
                        id.as_deref() == Some("lifecycle_record_producer")
                    }
                    (LifecycleProducer::Chain, "function-map.json") => {
                        id.as_deref() == Some("lifecycle_chain_record_producer")
                    }
                    (LifecycleProducer::Records, "mainline-call-map.json") => {
                        id.as_deref() == Some("lifecycle-record-production-v1")
                    }
                    (LifecycleProducer::Chain, "mainline-call-map.json") => {
                        id.as_deref() == Some("lifecycle-record-chain-production-v1")
                    }
                    (LifecycleProducer::Records, "verification-map.json") => {
                        matches!(
                            id.as_deref(),
                            Some("worktree_clean") | Some("baseline_reproduced")
                        )
                    }
                    (LifecycleProducer::Chain, "verification-map.json") => {
                        id.as_deref() == Some("lifecycle_chain_record_producer")
                            || entry
                                .get("required_for")
                                .and_then(Value::as_array)
                                .is_some_and(|uses| {
                                    uses.iter()
                                        .any(|use_case| use_case.as_str() == Some("promotion"))
                                })
                    }
                    _ => false,
                }
            })
            .collect();
        let compatible_projection = |candidate: &Value, expected: &Value| {
            if entry_id(candidate) != entry_id(expected) {
                return false;
            }
            let Some(expected_object) = expected.as_object() else {
                return false;
            };
            let Some(candidate_object) = candidate.as_object() else {
                return false;
            };
            expected_object.iter().all(|(key, value)| {
                match candidate_object.get(key) {
                    Some(actual) => actual == value,
                    // Relations are descriptive metadata. Older project
                    // projections may omit them, but a supplied value must
                    // remain canonical.
                    None => key == "relations",
                }
            })
        };
        for entry in &required_entries {
            if actual
                .iter()
                .filter(|candidate| compatible_projection(candidate, entry))
                .count()
                > 1
            {
                fail(format!("LIFECYCLE_PRODUCER_MAP_TAMPERED:{}", name));
            }
        }
        // A project map may extend unrelated entries, but an entry with a
        // producer-owned identity cannot shadow the canonical declaration.
        if actual.iter().any(|candidate| {
            required_entries
                .iter()
                .any(|entry| entry_id(candidate) == entry_id(entry))
                && !required_entries
                    .iter()
                    .any(|entry| compatible_projection(candidate, entry))
        }) {
            fail(format!("LIFECYCLE_PRODUCER_MAP_TAMPERED:{}", name));
        }
    }

    let registry_path = root.join(".appsdk/maps/module-registry.json");
    assert_no_symlink_components(root, &registry_path, "lifecycle_producer_module_registry");
    let registry: Value = serde_json::from_str(
        &fs::read_to_string(&registry_path)
            .unwrap_or_else(|_| fail("LIFECYCLE_PRODUCER_MODULE_REGISTRY_MISSING")),
    )
    .unwrap_or_else(|_| fail("LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID"));
    if registry.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail("LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID");
    }
    let registry_modules = registry
        .get("modules")
        .and_then(Value::as_array)
        .filter(|modules| !modules.is_empty())
        .unwrap_or_else(|| fail("LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID"));
    let project_module = project
        .get("modules")
        .and_then(Value::as_array)
        .and_then(|modules| {
            modules
                .iter()
                .find(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        })
        .unwrap_or_else(|| fail(format!("MODULE_NOT_FOUND:{}", module_id)));
    let project_owner = project_module
        .get("source_owner")
        .and_then(Value::as_str)
        .filter(|owner| !owner.is_empty())
        .unwrap_or_else(|| fail("LIFECYCLE_PRODUCER_MODULE_BINDING_MISMATCH"));
    let project_paths = project_module
        .get("owned_paths")
        .and_then(Value::as_array)
        .filter(|paths| !paths.is_empty())
        .unwrap_or_else(|| fail("LIFECYCLE_PRODUCER_MODULE_BINDING_MISMATCH"));
    let registry_binding = assert_registry_binding_contract(project_module, module_id);

    // A project module is a lifecycle scope and may intentionally aggregate
    // several finer-grained source modules. The registry is the source
    // ownership projection, so binding is established by active path coverage
    // rather than by requiring a registry entry with the same module_id.
    // Preserve the stronger identity check when such an entry does exist.
    let same_id_entries: Vec<&Value> = registry_modules
        .iter()
        .filter(|module| module.get("module_id").and_then(Value::as_str) == Some(module_id))
        .collect();
    if same_id_entries.len() > 1 {
        fail("LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID");
    }
    if let Some(registered) = same_id_entries.first() {
        if registered.get("status").and_then(Value::as_str) != Some("active")
            || registered.get("owner").and_then(Value::as_str) != Some(project_owner)
        {
            fail("LIFECYCLE_PRODUCER_MODULE_BINDING_MISMATCH");
        }
    }

    let coverage_modules: Vec<&Value> = match registry_binding {
        RegistryBinding::Exact => same_id_entries
            .first()
            .copied()
            .map(|registered| vec![registered])
            .unwrap_or_else(|| fail("LIFECYCLE_PRODUCER_MODULE_BINDING_MISSING")),
        RegistryBinding::Aggregate(selected_ids) => {
            if same_id_entries.first().is_some()
                && !selected_ids
                    .iter()
                    .any(|selected| selected.as_str() == Some(module_id))
            {
                fail("LIFECYCLE_PRODUCER_MODULE_BINDING_MISSING");
            }
            selected_ids
                .iter()
                .map(|selected| {
                    let selected_id = selected
                        .as_str()
                        .unwrap_or_else(|| fail("INVALID_REGISTRY_BINDING"));
                    let matches: Vec<&Value> = registry_modules
                        .iter()
                        .filter(|module| {
                            module.get("module_id").and_then(Value::as_str) == Some(selected_id)
                        })
                        .collect();
                    if matches.len() != 1 {
                        fail(format!(
                            "LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID:{}",
                            selected_id
                        ));
                    }
                    let registered = matches[0];
                    if registered.get("status").and_then(Value::as_str) != Some("active") {
                        fail(format!(
                            "LIFECYCLE_PRODUCER_MODULE_BINDING_MISMATCH:{}",
                            selected_id
                        ));
                    }
                    registered
                })
                .collect()
        }
    };
    let active_registry_paths: Vec<Vec<String>> = coverage_modules
        .iter()
        .map(|module| {
            let registry_module_id = module
                .get("module_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| fail("LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID"));
            let _registry_owner = module
                .get("owner")
                .and_then(Value::as_str)
                .filter(|owner| !owner.is_empty())
                .unwrap_or_else(|| {
                    fail(format!(
                        "LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID:{}",
                        registry_module_id
                    ))
                });
            record_array(module, "/owned_paths", "lifecycle_producer_module_registry")
                .iter()
                .map(|path| {
                    path.as_str()
                        .filter(|path| !path.is_empty())
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            fail(format!(
                                "LIFECYCLE_PRODUCER_MODULE_REGISTRY_INVALID:{}",
                                registry_module_id
                            ))
                        })
                })
                .collect()
        })
        .collect();
    for path in project_paths {
        let project_path = path
            .as_str()
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| fail("LIFECYCLE_PRODUCER_MODULE_BINDING_MISMATCH"));
        if !active_registry_paths.iter().any(|registered_paths| {
            registered_paths
                .iter()
                .any(|registered_path| registry_pattern_covers(registered_path, project_path))
        }) {
            fail("LIFECYCLE_PRODUCER_MODULE_BINDING_MISMATCH");
        }
    }
}

pub(super) fn registry_pattern_covers(registry_pattern: &str, project_pattern: &str) -> bool {
    let registry_is_recursive = registry_pattern.ends_with("/**");
    let project_is_recursive = project_pattern.ends_with("/**");
    let registry_root = registry_pattern
        .trim_end_matches("/**")
        .trim_end_matches('/');
    let project_root = project_pattern
        .trim_end_matches("/**")
        .trim_end_matches('/');
    if registry_root.is_empty() || project_root.is_empty() {
        return false;
    }
    if registry_is_recursive {
        project_root == registry_root || project_root.starts_with(&format!("{}/", registry_root))
    } else {
        !project_is_recursive && registry_root == project_root
    }
}

pub(super) fn registry_path_matches(pattern: &str, path: &str) -> bool {
    pattern
        .strip_suffix("/**")
        .map(|prefix| path == prefix || path.starts_with(&format!("{}/", prefix)))
        .unwrap_or(pattern == path)
}

pub(super) fn is_project_governance_path(path: &str) -> bool {
    path == "AGENTS.md"
        || path == ".appsdk-prepare.json"
        || path == ".appsdk"
        || path.starts_with(".appsdk/")
        || path == ".appsdk-control"
        || path.starts_with(".appsdk-control/")
        || path == ".agent-collab"
        || path.starts_with(".agent-collab/")
        || path == ".mcp.json"
        || path == ".codex/config.toml"
        || path == ".claude/settings.json"
        || path == "docs/collab.md"
        || path == "memory"
        || path.starts_with("memory/")
}

pub(super) fn has_valid_project_governance_contract(root: &Path) -> bool {
    let project = project_file(root);
    match fs::symlink_metadata(&project) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            fail("GOVERNANCE_PATH_SYMLINK:project")
        }
        Ok(metadata) if metadata.is_file() => {
            let value = read_project(root);
            assert_project_contract(root, &value);
            true
        }
        Ok(_) => fail("INVALID_PROJECT_CONTRACT"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => fail(format!("PROJECT_CONTRACT_MISSING:{}", project.display())),
    }
}

pub(super) fn assert_sdk_source_registry(root: &Path) {
    assert_project_root_safe(root);
    let governed_workspace = has_valid_project_governance_contract(root);
    let registry: Value = serde_json::from_str(
        &fs::read_to_string(root.join("contracts/maps/module-registry.json"))
            .unwrap_or_else(|_| fail("MISSING_SDK_MODULE_REGISTRY")),
    )
    .unwrap_or_else(|_| fail("INVALID_SDK_MODULE_REGISTRY"));
    let modules = record_array(&registry, "/modules", "module-registry.json");
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()
        .unwrap_or_else(|_| fail("SDK_SOURCE_REGISTRY_GIT_UNAVAILABLE"));
    if !output.status.success() {
        fail("SDK_SOURCE_REGISTRY_GIT_FAILED");
    }
    for bytes in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|v| !v.is_empty())
    {
        let path = std::str::from_utf8(bytes).unwrap_or_else(|_| fail("INVALID_SDK_SOURCE_PATH"));
        // An initialized workspace owns its project control plane through the
        // project contract; keep the SDK source registry focused on SDK source
        // paths. Without a valid project contract these names remain ordinary
        // source paths and are checked strictly below.
        if governed_workspace && is_project_governance_path(path) {
            continue;
        }
        let mut owners = Vec::new();
        for module in modules {
            let module_id = record_str(module, "/module_id", "module-registry.json");
            if module.get("status").and_then(Value::as_str) != Some("active") {
                continue;
            }
            if record_array(module, "/owned_paths", module_id)
                .iter()
                .any(|pattern| {
                    pattern
                        .as_str()
                        .is_some_and(|pattern| registry_path_matches(pattern, path))
                })
            {
                owners.push(module_id);
            }
            if record_array(module, "/forbidden_paths", module_id)
                .iter()
                .any(|pattern| {
                    pattern
                        .as_str()
                        .is_some_and(|pattern| registry_path_matches(pattern, path))
                })
            {
                fail(format!("SDK_SOURCE_FORBIDDEN_PATH:{}:{}", module_id, path));
            }
        }
        if owners.len() != 1 {
            fail(format!(
                "SDK_SOURCE_OWNER_CARDINALITY:{}:{}",
                path,
                owners.join(",")
            ));
        }
    }
    println!("{}", r#"{"ok":true,"gate":"sdk_source_registry"}"#);
}

pub(super) fn required_str<'a>(value: &'a Value, path: &str, error: &str) -> &'a str {
    try_required_str(value, path, error).unwrap_or_else(|error| fail(error))
}

pub(super) fn try_required_str<'a>(
    value: &'a Value,
    path: &str,
    error: &str,
) -> Result<&'a str, String> {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .ok_or_else(|| error.to_owned())
}

pub(super) fn assert_identifier(value: &str, error: &str) {
    try_assert_identifier(value, error).unwrap_or_else(|error| fail(error))
}

pub(super) fn try_assert_identifier(value: &str, error: &str) -> Result<(), String> {
    if value.is_empty()
        || !value.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        || !value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(error.to_owned());
    }
    Ok(())
}

pub(super) fn canonical(value: &Value) -> String {
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

pub(super) fn sha256(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

pub(super) fn assert_goal_confirmed(root: &Path) {
    assert_goal_contract(root, true);
}

pub(super) fn read_goal_if_present(root: &Path) -> Option<Value> {
    let file = root.join(".appsdk/goal.json");
    let metadata = match fs::symlink_metadata(&file) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(_) => fail("GOAL_RECORD_UNAVAILABLE"),
    };
    if metadata.file_type().is_symlink() {
        fail("GOVERNANCE_PATH_SYMLINK:goal");
    }
    let text = fs::read_to_string(file).unwrap_or_else(|_| fail("GOAL_RECORD_UNAVAILABLE"));
    let goal =
        serde_json::from_str(&text).unwrap_or_else(|_| fail("INVALID_GOAL_CLARIFICATION_RECORD"));
    crate::requirements::assert_requirements_current(root, &goal);
    Some(goal)
}

pub(super) fn read_goal(root: &Path) -> Value {
    read_goal_if_present(root).unwrap_or_else(|| fail("MISSING_GOAL_CLARIFICATION_RECORD"))
}

pub(super) fn assert_goal_contract(root: &Path, require_confirmed: bool) {
    let goal = read_goal(root);
    validate_goal_contract(&goal, require_confirmed);
}

pub(super) fn assert_goal_contract_if_present(root: &Path) {
    if let Some(goal) = read_goal_if_present(root) {
        validate_goal_contract(&goal, false);
    }
}

pub(super) fn validate_goal_contract(goal: &Value, require_confirmed: bool) {
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
