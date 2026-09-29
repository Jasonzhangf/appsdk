use super::*;

const TEST_GOVERNANCE_RESULT_ROOT: &str = ".appsdk/records/test-scenario-results";

pub(super) fn optional_test_governance_selection(
    project: &Value,
) -> Result<Option<&Value>, String> {
    let Some(selection) = project.get("test_governance") else {
        return Ok(None);
    };
    let object = selection
        .as_object()
        .ok_or_else(|| "INVALID_TEST_GOVERNANCE_SELECTION".to_string())?;
    if object.keys().any(|key| key != "mode" && key != "manifest") {
        return Err("INVALID_TEST_GOVERNANCE_SELECTION".to_string());
    }
    let mode = object
        .get("mode")
        .and_then(Value::as_str)
        .ok_or_else(|| "INVALID_TEST_GOVERNANCE_SELECTION".to_string())?;
    match mode {
        "off" => Ok(None),
        "selected" => {
            if object
                .get("manifest")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                return Err("TEST_GOVERNANCE_MANIFEST_REQUIRED".to_string());
            }
            Ok(Some(selection))
        }
        _ => Err("INVALID_TEST_GOVERNANCE_MODE".to_string()),
    }
}

fn read_json_file(root: &Path, path: &Path, error: &str) -> Result<Value, String> {
    assert_no_symlink_components(root, path, error);
    let metadata = fs::symlink_metadata(path).map_err(|_| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(error.to_string());
    }
    let bytes = fs::read(path).map_err(|_| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| error.to_string())
}

fn validate_record_schema(schema_relative: &str, record: &Value) -> Result<(), String> {
    let schema = canonical_record_contract(schema_relative);
    let validator = jsonschema::validator_for(&schema)
        .map_err(|_| format!("INVALID_RECORD_SCHEMA:{}", schema_relative))?;
    validator
        .validate(record)
        .map_err(|_| format!("INVALID_RECORD:{}", schema_relative))
}

fn test_governance_string<'a>(
    value: &'a Value,
    pointer: &str,
    error: &str,
) -> Result<&'a str, String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|entry| !entry.is_empty())
        .ok_or_else(|| error.to_string())
}

fn test_governance_non_command_string<'a>(
    value: &'a Value,
    pointer: &str,
    error: &str,
) -> Result<&'a str, String> {
    let value = test_governance_string(value, pointer, error)?;
    if value.chars().any(|character| {
        matches!(
            character,
            ';' | '&'
                | '|'
                | '<'
                | '>'
                | '('
                | ')'
                | '`'
                | '$'
                | '\\'
                | '\''
                | '"'
                | '['
                | ']'
                | '{'
                | '}'
                | '!'
                | '*'
                | '?'
                | '~'
                | '#'
                | '\n'
                | '\r'
                | '\t'
        )
    }) {
        return Err(error.to_string());
    }
    Ok(value)
}

fn test_governance_identifier(value: &Value, pointer: &str, error: &str) -> Result<(), String> {
    let value = test_governance_string(value, pointer, error)?;
    if value.len() < 2
        || !value.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        || !value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(error.to_string());
    }
    Ok(())
}

fn test_governance_safe_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters.next().is_some_and(|c| c.is_ascii_lowercase())
        && characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn test_governance_evidence_path(
    root: &Path,
    object_id: &str,
    evidence_id: &str,
) -> Option<PathBuf> {
    if !test_governance_safe_identifier(object_id) || !test_governance_safe_identifier(evidence_id)
    {
        return None;
    }
    Some(
        root.join(".appsdk")
            .join("records")
            .join("evidence")
            .join(object_id)
            .join(format!("{}.json", evidence_id)),
    )
}

fn test_governance_datetime(value: &str, error: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| error.to_string())
}

struct TestGovernanceSemanticGraph {
    entry: String,
    exit: String,
    nodes: BTreeSet<String>,
    edges: BTreeSet<(String, String)>,
}

fn test_governance_has_chinese(value: &str) -> bool {
    value.chars().any(|character| {
        matches!(
            character,
            '\u{3400}'..='\u{4DBF}'
                | '\u{4E00}'..='\u{9FFF}'
                | '\u{F900}'..='\u{FAFF}'
        )
    })
}

fn test_governance_semantic_graph(
    object: &Value,
    object_id: &str,
) -> Result<TestGovernanceSemanticGraph, String> {
    let semantic_graph = object
        .get("semantic_graph")
        .filter(|value| value.is_object())
        .ok_or_else(|| "TEST_GOVERNANCE_SEMANTIC_GRAPH_REQUIRED".to_string())?;
    let entry = test_governance_string(
        semantic_graph,
        "/entry",
        &format!("TEST_GOVERNANCE_SEMANTIC_GRAPH_ENTRY_REQUIRED:{object_id}"),
    )?
    .to_string();
    let exit = test_governance_string(
        semantic_graph,
        "/exit",
        &format!("TEST_GOVERNANCE_SEMANTIC_GRAPH_EXIT_REQUIRED:{object_id}"),
    )?
    .to_string();
    let nodes = semantic_graph
        .get("nodes")
        .and_then(Value::as_array)
        .filter(|nodes| !nodes.is_empty())
        .ok_or_else(|| format!("TEST_GOVERNANCE_SEMANTIC_GRAPH_NODES_REQUIRED:{object_id}"))?;
    let mut node_ids = BTreeSet::new();
    for node in nodes {
        let node_id = test_governance_string(
            node,
            "/id",
            &format!("TEST_GOVERNANCE_SEMANTIC_GRAPH_NODE_INVALID:{object_id}"),
        )?
        .to_string();
        let label = test_governance_string(
            node,
            "/label",
            &format!("TEST_GOVERNANCE_SEMANTIC_GRAPH_NODE_LABEL_REQUIRED:{object_id}:{node_id}"),
        )?;
        if !test_governance_has_chinese(label) {
            return Err(format!(
                "TEST_GOVERNANCE_SEMANTIC_GRAPH_NODE_LABEL_NOT_CHINESE:{object_id}:{node_id}"
            ));
        }
        if !node_ids.insert(node_id.clone()) {
            return Err(format!(
                "TEST_GOVERNANCE_SEMANTIC_GRAPH_NODE_DUPLICATE:{object_id}:{node_id}"
            ));
        }
    }
    if !node_ids.contains(&entry) {
        return Err(format!(
            "TEST_GOVERNANCE_SEMANTIC_GRAPH_ENTRY_UNDEFINED:{object_id}:{entry}"
        ));
    }
    if !node_ids.contains(&exit) {
        return Err(format!(
            "TEST_GOVERNANCE_SEMANTIC_GRAPH_EXIT_UNDEFINED:{object_id}:{exit}"
        ));
    }
    let edges = semantic_graph
        .get("edges")
        .and_then(Value::as_array)
        .filter(|edges| !edges.is_empty())
        .ok_or_else(|| format!("TEST_GOVERNANCE_SEMANTIC_GRAPH_EDGES_REQUIRED:{object_id}"))?;
    let mut edge_set = BTreeSet::new();
    for edge in edges {
        let from = test_governance_string(
            edge,
            "/from",
            &format!("TEST_GOVERNANCE_SEMANTIC_GRAPH_EDGE_INVALID:{object_id}"),
        )?
        .to_string();
        let to = test_governance_string(
            edge,
            "/to",
            &format!("TEST_GOVERNANCE_SEMANTIC_GRAPH_EDGE_INVALID:{object_id}"),
        )?
        .to_string();
        if !node_ids.contains(&from) || !node_ids.contains(&to) {
            return Err(format!(
                "TEST_GOVERNANCE_SEMANTIC_GRAPH_EDGE_UNDEFINED:{object_id}:{from}->{to}"
            ));
        }
        edge_set.insert((from, to));
    }
    let indegree_zero = node_ids
        .iter()
        .filter(|node_id| !edge_set.iter().any(|(_, to)| to == *node_id))
        .collect::<Vec<_>>();
    let outdegree_zero = node_ids
        .iter()
        .filter(|node_id| !edge_set.iter().any(|(from, _)| from == *node_id))
        .collect::<Vec<_>>();
    if indegree_zero.len() != 1 {
        return Err(format!(
            "TEST_GOVERNANCE_SEMANTIC_GRAPH_NOT_SESE_MULTI_ENTRY:{object_id}"
        ));
    }
    if outdegree_zero.len() != 1 {
        return Err(format!(
            "TEST_GOVERNANCE_SEMANTIC_GRAPH_NOT_SESE_MULTI_EXIT:{object_id}"
        ));
    }
    if indegree_zero[0] != &entry || outdegree_zero[0] != &exit {
        return Err(format!(
            "TEST_GOVERNANCE_SEMANTIC_GRAPH_ENTRY_EXIT_MISMATCH:{object_id}"
        ));
    }

    let mut reachable = BTreeSet::new();
    reachable.insert(entry.clone());
    let mut frontier = vec![entry.clone()];
    while let Some(node) = frontier.pop() {
        for (_, to) in edge_set.iter().filter(|(from, _)| *from == node) {
            if reachable.insert(to.clone()) {
                frontier.push(to.clone());
            }
        }
    }
    let mut reaches_exit = BTreeSet::new();
    reaches_exit.insert(exit.clone());
    let mut frontier = vec![exit.clone()];
    while let Some(node) = frontier.pop() {
        for (from, _) in edge_set.iter().filter(|(_, to)| *to == node) {
            if reaches_exit.insert(from.clone()) {
                frontier.push(from.clone());
            }
        }
    }
    for node_id in &node_ids {
        if !reachable.contains(node_id) {
            return Err(format!(
                "TEST_GOVERNANCE_SEMANTIC_GRAPH_UNREACHABLE_NODE:{object_id}:{node_id}"
            ));
        }
        if !reaches_exit.contains(node_id) {
            return Err(format!(
                "TEST_GOVERNANCE_SEMANTIC_GRAPH_NODE_NO_EXIT:{object_id}:{node_id}"
            ));
        }
    }
    Ok(TestGovernanceSemanticGraph {
        entry,
        exit,
        nodes: node_ids,
        edges: edge_set,
    })
}

fn test_governance_scenario_semantic_path(
    scenario: &Value,
    object_id: &str,
    scenario_id: &str,
    graph: &TestGovernanceSemanticGraph,
) -> Result<(), String> {
    let path = scenario
        .get("path_node_ids")
        .and_then(Value::as_array)
        .filter(|path| path.len() >= 2)
        .ok_or_else(|| {
            format!("TEST_GOVERNANCE_SCENARIO_PATH_REQUIRED:{object_id}:{scenario_id}")
        })?;
    let mut node_ids = Vec::new();
    for (index, value) in path.iter().enumerate() {
        let node_id = value
            .as_str()
            .filter(|node_id| !node_id.is_empty())
            .ok_or_else(|| {
                format!("TEST_GOVERNANCE_SCENARIO_PATH_INVALID:{object_id}:{scenario_id}:{index}")
            })?;
        if !graph.nodes.contains(node_id) {
            return Err(format!(
                "TEST_GOVERNANCE_SCENARIO_PATH_NODE_UNDEFINED:{object_id}:{scenario_id}:{node_id}"
            ));
        }
        node_ids.push(node_id.to_string());
    }
    if node_ids.first() != Some(&graph.entry) {
        return Err(format!(
            "TEST_GOVERNANCE_SCENARIO_PATH_ENTRY_MISMATCH:{object_id}:{scenario_id}"
        ));
    }
    if node_ids.last() != Some(&graph.exit) {
        return Err(format!(
            "TEST_GOVERNANCE_SCENARIO_PATH_EXIT_MISMATCH:{object_id}:{scenario_id}"
        ));
    }
    for pair in node_ids.windows(2) {
        if !graph.edges.contains(&(pair[0].clone(), pair[1].clone())) {
            return Err(format!(
                "TEST_GOVERNANCE_SCENARIO_PATH_EDGE_MISSING:{object_id}:{scenario_id}:{}->{}",
                pair[0], pair[1]
            ));
        }
    }
    Ok(())
}

fn validate_test_governance_manifest(
    root: &Path,
    project: &Value,
    selection: &Value,
) -> Result<Value, String> {
    let manifest_relative =
        test_governance_string(selection, "/manifest", "TEST_GOVERNANCE_MANIFEST_REQUIRED")?;
    let manifest_path = safe_owned_path(root, manifest_relative, "test_governance_manifest");
    let manifest = read_json_file(root, &manifest_path, "TEST_GOVERNANCE_MANIFEST_UNAVAILABLE")?;
    if manifest.get("schema_version").and_then(Value::as_u64) != Some(1)
        || manifest.get("mode").and_then(Value::as_str) != Some("selected")
    {
        return Err("INVALID_TEST_GOVERNANCE_MANIFEST".to_string());
    }
    let objects = manifest
        .get("objects")
        .and_then(Value::as_array)
        .filter(|objects| !objects.is_empty())
        .ok_or_else(|| "TEST_GOVERNANCE_OBJECTS_REQUIRED".to_string())?;
    let runners = manifest
        .get("trusted_runners")
        .and_then(Value::as_array)
        .filter(|runners| !runners.is_empty())
        .ok_or_else(|| "TEST_GOVERNANCE_TRUSTED_RUNNERS_REQUIRED".to_string())?;
    let mut runner_refs = BTreeSet::new();
    for runner in runners {
        test_governance_identifier(runner, "/runner_ref", "INVALID_TEST_GOVERNANCE_RUNNER")?;
        test_governance_non_command_string(
            runner,
            "/entrypoint",
            "INVALID_TEST_GOVERNANCE_RUNNER",
        )?;
        test_governance_string(runner, "/owner", "INVALID_TEST_GOVERNANCE_RUNNER")?;
        if !runner_refs.insert(
            test_governance_string(runner, "/runner_ref", "INVALID_TEST_GOVERNANCE_RUNNER")?
                .to_string(),
        ) {
            return Err("DUPLICATE_TEST_GOVERNANCE_RUNNER".to_string());
        }
    }
    let authorizations = manifest
        .get("effect_authorizations")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut authorization_ids = BTreeSet::new();
    for authorization in authorizations {
        test_governance_identifier(
            authorization,
            "/authorization_id",
            "INVALID_TEST_GOVERNANCE_AUTHORIZATION",
        )?;
        test_governance_identifier(
            authorization,
            "/object_id",
            "INVALID_TEST_GOVERNANCE_AUTHORIZATION",
        )?;
        test_governance_identifier(
            authorization,
            "/scenario_id",
            "INVALID_TEST_GOVERNANCE_AUTHORIZATION",
        )?;
        test_governance_string(
            authorization,
            "/environment",
            "INVALID_TEST_GOVERNANCE_AUTHORIZATION",
        )?;
        let allowed_effects = authorization
            .get("allowed_effects")
            .and_then(Value::as_array)
            .filter(|effects| !effects.is_empty())
            .ok_or_else(|| "INVALID_TEST_GOVERNANCE_AUTHORIZATION".to_string())?;
        if allowed_effects.iter().any(|effect| {
            effect
                .as_str()
                .filter(|effect| !effect.is_empty())
                .is_none()
        }) {
            return Err("INVALID_TEST_GOVERNANCE_AUTHORIZATION".to_string());
        }
        let valid_from = test_governance_datetime(
            test_governance_string(
                authorization,
                "/valid_from",
                "INVALID_TEST_GOVERNANCE_AUTHORIZATION",
            )?,
            "INVALID_TEST_GOVERNANCE_AUTHORIZATION",
        )?;
        let valid_until = test_governance_datetime(
            test_governance_string(
                authorization,
                "/valid_until",
                "INVALID_TEST_GOVERNANCE_AUTHORIZATION",
            )?,
            "INVALID_TEST_GOVERNANCE_AUTHORIZATION",
        )?;
        if valid_from > valid_until {
            return Err("INVALID_TEST_GOVERNANCE_AUTHORIZATION".to_string());
        }
        test_governance_string(
            authorization,
            "/approval_ref",
            "INVALID_TEST_GOVERNANCE_AUTHORIZATION",
        )?;
        if !authorization_ids.insert(
            test_governance_string(
                authorization,
                "/authorization_id",
                "INVALID_TEST_GOVERNANCE_AUTHORIZATION",
            )?
            .to_string(),
        ) {
            return Err("DUPLICATE_TEST_GOVERNANCE_AUTHORIZATION".to_string());
        }
    }

    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .ok_or_else(|| "INVALID_PROJECT_CONTRACT:/modules".to_string())?;
    let module_ids = modules
        .iter()
        .filter_map(|module| module.get("module_id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let mut object_ids = BTreeSet::new();
    for object in objects {
        let object_id =
            test_governance_string(object, "/object_id", "INVALID_TEST_GOVERNANCE_OBJECT")?;
        test_governance_identifier(object, "/object_id", "INVALID_TEST_GOVERNANCE_OBJECT")?;
        if !module_ids.contains(object_id) {
            return Err(format!("TEST_GOVERNANCE_OBJECT_NOT_FOUND:{}", object_id));
        }
        if !object_ids.insert(object_id.to_string()) {
            return Err(format!("DUPLICATE_TEST_GOVERNANCE_OBJECT:{}", object_id));
        }
        test_governance_string(object, "/graph_id", "INVALID_TEST_GOVERNANCE_OBJECT")?;
        test_governance_string(object, "/graph_version", "INVALID_TEST_GOVERNANCE_OBJECT")?;
        let confirmation = object
            .get("scope_confirmation")
            .ok_or_else(|| "TEST_GOVERNANCE_SCOPE_CONFIRMATION_REQUIRED".to_string())?;
        test_governance_string(
            confirmation,
            "/reference",
            "INVALID_TEST_GOVERNANCE_SCOPE_CONFIRMATION",
        )?;
        test_governance_string(
            confirmation,
            "/confirmed_by",
            "INVALID_TEST_GOVERNANCE_SCOPE_CONFIRMATION",
        )?;
        test_governance_datetime(
            test_governance_string(
                confirmation,
                "/confirmed_at",
                "INVALID_TEST_GOVERNANCE_SCOPE_CONFIRMATION",
            )?,
            "INVALID_TEST_GOVERNANCE_SCOPE_CONFIRMATION",
        )?;
        let semantic_graph = test_governance_semantic_graph(object, object_id)?;
        let scenarios = object
            .get("scenarios")
            .and_then(Value::as_array)
            .filter(|scenarios| !scenarios.is_empty())
            .ok_or_else(|| "TEST_GOVERNANCE_SCENARIOS_REQUIRED".to_string())?;
        let mut scenario_ids = BTreeSet::new();
        for scenario in scenarios {
            let scenario_id = test_governance_string(
                scenario,
                "/scenario_id",
                "INVALID_TEST_GOVERNANCE_SCENARIO",
            )?;
            test_governance_identifier(
                scenario,
                "/scenario_id",
                "INVALID_TEST_GOVERNANCE_SCENARIO",
            )?;
            if !scenario_ids.insert(scenario_id.to_string()) {
                return Err(format!(
                    "DUPLICATE_TEST_GOVERNANCE_SCENARIO:{}:{}",
                    object_id, scenario_id
                ));
            }
            for field in ["/semantic_name", "/stimulus", "/runner_ref"] {
                test_governance_string(scenario, field, "INVALID_TEST_GOVERNANCE_SCENARIO")?;
            }
            for field in ["/entrypoint", "/cleanup"] {
                test_governance_non_command_string(
                    scenario,
                    field,
                    "INVALID_TEST_GOVERNANCE_SCENARIO",
                )?;
            }
            let runner_ref = test_governance_string(
                scenario,
                "/runner_ref",
                "INVALID_TEST_GOVERNANCE_SCENARIO",
            )?;
            if !runner_refs.contains(runner_ref) {
                return Err(format!("TEST_GOVERNANCE_RUNNER_NOT_TRUSTED:{}", runner_ref));
            }
            test_governance_scenario_semantic_path(
                scenario,
                object_id,
                scenario_id,
                &semantic_graph,
            )?;
            for field in ["/preconditions", "/observable_assertions"] {
                let values = scenario
                    .get(field.trim_start_matches('/'))
                    .and_then(Value::as_array)
                    .filter(|values| !values.is_empty())
                    .ok_or_else(|| "INVALID_TEST_GOVERNANCE_SCENARIO".to_string())?;
                if values
                    .iter()
                    .any(|value| value.as_str().filter(|value| !value.is_empty()).is_none())
                {
                    return Err("INVALID_TEST_GOVERNANCE_SCENARIO".to_string());
                }
            }
            let expected_effects = scenario
                .get("expected_effects")
                .and_then(Value::as_array)
                .ok_or_else(|| "INVALID_TEST_GOVERNANCE_SCENARIO".to_string())?;
            if expected_effects
                .iter()
                .any(|value| value.as_str().filter(|value| !value.is_empty()).is_none())
            {
                return Err("INVALID_TEST_GOVERNANCE_SCENARIO".to_string());
            }
            let classification = scenario
                .get("classification")
                .and_then(Value::as_array)
                .filter(|values| !values.is_empty())
                .ok_or_else(|| "INVALID_TEST_GOVERNANCE_SCENARIO".to_string())?;
            let mut seen_classification = BTreeSet::new();
            for value in classification {
                let value = value
                    .as_str()
                    .filter(|value| {
                        matches!(*value, "normal" | "negative" | "lifecycle" | "effect")
                    })
                    .ok_or_else(|| "INVALID_TEST_GOVERNANCE_SCENARIO".to_string())?;
                if !seen_classification.insert(value) {
                    return Err("INVALID_TEST_GOVERNANCE_SCENARIO".to_string());
                }
            }
            if seen_classification.contains("effect") {
                let authorization_id = test_governance_string(
                    scenario,
                    "/effect_authorization_id",
                    "TEST_GOVERNANCE_EFFECT_AUTHORIZATION_REQUIRED",
                )?;
                let authorization = authorizations
                    .iter()
                    .find(|authorization| {
                        authorization
                            .get("authorization_id")
                            .and_then(Value::as_str)
                            == Some(authorization_id)
                    })
                    .ok_or_else(|| {
                        format!(
                            "TEST_GOVERNANCE_EFFECT_AUTHORIZATION_NOT_FOUND:{}",
                            authorization_id
                        )
                    })?;
                if authorization.get("object_id").and_then(Value::as_str) != Some(object_id)
                    || authorization.get("scenario_id").and_then(Value::as_str) != Some(scenario_id)
                {
                    return Err(format!(
                        "TEST_GOVERNANCE_EFFECT_AUTHORIZATION_MISMATCH:{}",
                        authorization_id
                    ));
                }
                let allowed_effects = authorization
                    .get("allowed_effects")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "INVALID_TEST_GOVERNANCE_AUTHORIZATION".to_string())?;
                if expected_effects.iter().any(|effect| {
                    !allowed_effects
                        .iter()
                        .any(|allowed| allowed.as_str() == effect.as_str())
                }) {
                    return Err(format!(
                        "TEST_GOVERNANCE_EFFECT_AUTHORIZATION_SCOPE_MISMATCH:{}",
                        authorization_id
                    ));
                }
            }
        }
        let invariants = object
            .get("invariants")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut invariant_ids = BTreeSet::new();
        for invariant in invariants {
            let invariant_id = test_governance_string(
                invariant,
                "/invariant_id",
                "INVALID_TEST_GOVERNANCE_INVARIANT",
            )?;
            test_governance_identifier(
                invariant,
                "/invariant_id",
                "INVALID_TEST_GOVERNANCE_INVARIANT",
            )?;
            if !invariant_ids.insert(invariant_id.to_string()) {
                return Err(format!(
                    "DUPLICATE_TEST_GOVERNANCE_INVARIANT:{}:{}",
                    object_id, invariant_id
                ));
            }
            test_governance_string(
                invariant,
                "/semantic_name",
                "INVALID_TEST_GOVERNANCE_INVARIANT",
            )?;
            let applies_to_scenarios = invariant
                .get("applies_to_scenarios")
                .and_then(Value::as_array)
                .filter(|values| !values.is_empty())
                .ok_or_else(|| {
                    format!(
                        "TEST_GOVERNANCE_INVARIANT_SCENARIOS_REQUIRED:{}:{}",
                        object_id, invariant_id
                    )
                })?;
            let mut seen_scenarios = BTreeSet::new();
            for scenario_value in applies_to_scenarios {
                let scenario_id = scenario_value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "INVALID_TEST_GOVERNANCE_INVARIANT".to_string())?;
                if !seen_scenarios.insert(scenario_id.to_string()) {
                    return Err("INVALID_TEST_GOVERNANCE_INVARIANT".to_string());
                }
                if !scenario_ids.contains(scenario_id) {
                    return Err(format!(
                        "TEST_GOVERNANCE_INVARIANT_SCENARIO_NOT_FOUND:{}:{}:{}",
                        object_id, invariant_id, scenario_id
                    ));
                }
            }
        }
    }
    if validate_record_schema("contracts/test-governance.schema.json", &manifest).is_err() {
        return Err("INVALID_TEST_GOVERNANCE_MANIFEST".to_string());
    }
    Ok(manifest)
}

fn test_governance_object_status(
    root: &Path,
    object: &Value,
    candidate_commit: &str,
    manifest: &Value,
    now: DateTime<Utc>,
) -> Value {
    let object_id = object
        .get("object_id")
        .and_then(Value::as_str)
        .unwrap_or("invalid-object");
    let graph_id = object.get("graph_id").and_then(Value::as_str).unwrap_or("");
    let graph_version = object
        .get("graph_version")
        .and_then(Value::as_str)
        .unwrap_or("");
    let scenarios = object
        .get("scenarios")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let authorizations = manifest
        .get("effect_authorizations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let invariants = object
        .get("invariants")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut scenario_reports = Vec::new();
    let mut invariant_reports = Vec::new();
    let mut object_blocked = false;
    for scenario in scenarios {
        let scenario_id = scenario
            .get("scenario_id")
            .and_then(Value::as_str)
            .unwrap_or("invalid-scenario");
        let result_relative = format!(
            "{}/{}/{}.json",
            TEST_GOVERNANCE_RESULT_ROOT, object_id, scenario_id
        );
        let result_path = root.join(&result_relative);
        let result = match read_json_file(root, &result_path, "TEST_GOVERNANCE_RESULT_UNAVAILABLE")
        {
            Ok(result) => result,
            Err(_) => {
                object_blocked = true;
                scenario_reports.push(serde_json::json!({
                    "scenario_id": scenario_id,
                    "status": "blocked",
                    "reason": "result_missing"
                }));
                continue;
            }
        };
        if validate_record_schema(
            "contracts/records/test-scenario-result-record.schema.json",
            &result,
        )
        .is_err()
        {
            object_blocked = true;
            scenario_reports.push(serde_json::json!({
                "scenario_id": scenario_id,
                "status": "blocked",
                "reason": "result_schema_invalid"
            }));
            continue;
        }
        let mut reason = None;
        let status = result
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("blocked");
        if result.get("object_id").and_then(Value::as_str) != Some(object_id)
            || result.get("scenario_id").and_then(Value::as_str) != Some(scenario_id)
            || result.get("graph_id").and_then(Value::as_str) != Some(graph_id)
            || result.get("graph_version").and_then(Value::as_str) != Some(graph_version)
        {
            reason = Some("identity_mismatch");
        }
        if result.get("candidate_commit").and_then(Value::as_str) != Some(candidate_commit) {
            reason = Some("candidate_commit_mismatch");
        }
        if result.get("entrypoint").and_then(Value::as_str)
            != scenario.get("entrypoint").and_then(Value::as_str)
        {
            reason = Some("entrypoint_mismatch");
        }
        let cleanup_status = result
            .pointer("/cleanup_result/status")
            .and_then(Value::as_str)
            .unwrap_or("blocked");
        if cleanup_status == "failed" || cleanup_status == "blocked" {
            reason = Some("cleanup_failed");
        } else {
            let cleanup_detail = result
                .pointer("/cleanup_result/detail")
                .and_then(Value::as_str)
                .unwrap_or("");
            if cleanup_detail.is_empty()
                || cleanup_detail.chars().any(|character| {
                    matches!(
                        character,
                        ';' | '\n'
                            | '|'
                            | '<'
                            | '>'
                            | '('
                            | ')'
                            | '`'
                            | '$'
                            | '\\'
                            | '\''
                            | '"'
                            | '['
                            | ']'
                            | '{'
                            | '}'
                            | '!'
                            | '*'
                            | '?'
                            | '~'
                            | '#'
                            | '\r'
                            | '\t'
                    )
                })
            {
                reason = Some("cleanup_failed");
            }
        }
        let environment = result
            .get("environment")
            .and_then(Value::as_str)
            .unwrap_or("");
        let mut authorization_checked = false;
        let classification_has_effect = scenario
            .get("classification")
            .and_then(Value::as_array)
            .map(|values| values.iter().any(|value| value.as_str() == Some("effect")))
            .unwrap_or(false);
        if classification_has_effect {
            let authorization_id = result
                .get("effect_authorization_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if scenario
                .get("effect_authorization_id")
                .and_then(Value::as_str)
                != Some(authorization_id)
            {
                reason = Some("effect_authorization_mismatch");
            }
            let authorization = authorizations.iter().find(|authorization| {
                authorization
                    .get("authorization_id")
                    .and_then(Value::as_str)
                    == Some(authorization_id)
            });
            let Some(authorization) = authorization else {
                reason = Some("effect_authorization_missing");
                scenario_reports.push(serde_json::json!({
                    "scenario_id": scenario_id,
                    "status": "blocked",
                    "reason": reason
                }));
                object_blocked = true;
                continue;
            };
            if authorization.get("object_id").and_then(Value::as_str) != Some(object_id)
                || authorization.get("scenario_id").and_then(Value::as_str) != Some(scenario_id)
                || authorization.get("environment").and_then(Value::as_str) != Some(environment)
            {
                reason = Some("effect_authorization_mismatch");
            }
            let valid_from = authorization
                .get("valid_from")
                .and_then(Value::as_str)
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc));
            let valid_until = authorization
                .get("valid_until")
                .and_then(Value::as_str)
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc));
            let started_at = result
                .get("started_at")
                .and_then(Value::as_str)
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc));
            let finished_at = result
                .get("finished_at")
                .and_then(Value::as_str)
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc));
            if valid_from.is_none()
                || valid_until.is_none()
                || started_at.is_none()
                || finished_at.is_none()
                || started_at.unwrap() > finished_at.unwrap()
                || finished_at.unwrap() > now
                || valid_from.unwrap() > started_at.unwrap()
                || finished_at.unwrap() > valid_until.unwrap()
                || now > valid_until.unwrap()
            {
                reason = Some("effect_authorization_expired");
            }
            authorization_checked = true;
        }
        if status == "passed" {
            let evidence_id = result.get("evidence_id").and_then(Value::as_str);
            if evidence_id.is_none() {
                reason = Some("evidence_missing");
            } else if let Some(evidence_id) = evidence_id {
                let Some(evidence_path) =
                    test_governance_evidence_path(root, object_id, evidence_id)
                else {
                    reason = Some("evidence_mismatch");
                    object_blocked = true;
                    scenario_reports.push(serde_json::json!({
                        "scenario_id": scenario_id,
                        "status": "blocked",
                        "reason": reason
                    }));
                    continue;
                };
                let evidence = match read_json_file(
                    root,
                    &evidence_path,
                    "TEST_GOVERNANCE_EVIDENCE_UNAVAILABLE",
                ) {
                    Ok(evidence) => evidence,
                    Err(_) => {
                        reason = Some("evidence_missing");
                        scenario_reports.push(serde_json::json!({
                            "scenario_id": scenario_id,
                            "status": "blocked",
                            "reason": reason
                        }));
                        object_blocked = true;
                        continue;
                    }
                };
                if validate_record_schema(
                    "contracts/records/evidence-record.schema.json",
                    &evidence,
                )
                .is_err()
                {
                    reason = Some("evidence_schema_invalid");
                } else if evidence.get("evidence_id").and_then(Value::as_str) != Some(evidence_id)
                    || evidence.get("source_commit").and_then(Value::as_str)
                        != Some(candidate_commit)
                    || evidence.get("result").and_then(Value::as_str) != Some("pass")
                    || evidence
                        .get("artifact_hash")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .is_none()
                    || evidence.get("environment_id").and_then(Value::as_str) != Some(environment)
                    || evidence.get("entrypoint").and_then(Value::as_str)
                        != scenario.get("entrypoint").and_then(Value::as_str)
                    || evidence.get("phase").and_then(Value::as_str) != Some("deployed_blackbox")
                    || evidence.get("execution_surface").and_then(Value::as_str)
                        != Some("deployed_blackbox")
                    || !matches!(
                        evidence.get("kind").and_then(Value::as_str),
                        Some("runtime" | "sample_replay")
                    )
                    || result.get("producer") != evidence.get("producer")
                    || evidence.pointer("/scope/module_id").and_then(Value::as_str)
                        != Some(object_id)
                {
                    reason = Some("evidence_mismatch");
                } else {
                    let created_at = evidence
                        .get("created_at")
                        .and_then(Value::as_str)
                        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                        .map(|value| value.with_timezone(&Utc));
                    let expires_at = evidence
                        .get("expires_at")
                        .and_then(Value::as_str)
                        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                        .map(|value| value.with_timezone(&Utc));
                    if created_at.is_none()
                        || expires_at.is_none()
                        || now < created_at.unwrap()
                        || created_at.unwrap() > expires_at.unwrap()
                        || now > expires_at.unwrap()
                    {
                        reason = Some("evidence_expired");
                    }
                }
            }
        } else {
            reason = Some(match status {
                "planned" => "planned",
                "failed" => "failed",
                "blocked" => "blocked",
                _ => "invalid_status",
            });
        }
        if !authorization_checked && classification_has_effect {
            reason = Some("effect_authorization_missing");
        }
        if reason.is_some() {
            object_blocked = true;
        }
        scenario_reports.push(serde_json::json!({
            "scenario_id": scenario_id,
            "status": if reason.is_none() { "passed" } else { "blocked" },
            "reason": reason
        }));
    }
    for invariant in invariants {
        let invariant_id = invariant
            .get("invariant_id")
            .and_then(Value::as_str)
            .unwrap_or("invalid-invariant");
        let semantic_name = invariant
            .get("semantic_name")
            .and_then(Value::as_str)
            .unwrap_or("");
        let covered_by = invariant
            .get("applies_to_scenarios")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|scenario_id| {
                scenario_reports.iter().any(|report| {
                    report.get("scenario_id").and_then(Value::as_str) == Some(scenario_id)
                        && report.get("status").and_then(Value::as_str) == Some("passed")
                })
            })
            .map(str::to_string)
            .collect::<Vec<_>>();
        let covered = !covered_by.is_empty();
        if !covered {
            object_blocked = true;
        }
        invariant_reports.push(serde_json::json!({
            "invariant_id": invariant_id,
            "semantic_name": semantic_name,
            "status": if covered { "covered" } else { "blocked" },
            "covered_by": covered_by,
            "reason": if covered {
                Value::Null
            } else {
                Value::String("invariant_not_covered".to_string())
            }
        }));
    }
    serde_json::json!({
        "object_id": object_id,
        "graph_id": graph_id,
        "graph_version": graph_version,
        "status": if object_blocked { "blocked" } else { "passed" },
        "scenarios": scenario_reports,
        "invariants": invariant_reports
    })
}

pub(super) fn test_governance_report(
    root: &Path,
    project: &Value,
    object_filter: Option<&str>,
    strict: bool,
) -> Result<Value, String> {
    let Some(selection) = optional_test_governance_selection(project)? else {
        if let Some(object_id) = object_filter {
            return Ok(serde_json::json!({
                "mode": "off",
                "status": "not_selected",
                "objects": [{"object_id": object_id, "status": "not_selected"}]
            }));
        }
        return Ok(serde_json::json!({
            "mode": "off",
            "status": "not_selected",
            "objects": []
        }));
    };
    let manifest = validate_test_governance_manifest(root, project, selection)?;
    let candidate_commit = git_value(
        root,
        &["rev-parse", "HEAD"],
        "TEST_GOVERNANCE_CANDIDATE_COMMIT_UNAVAILABLE",
    );
    let objects = manifest
        .get("objects")
        .and_then(Value::as_array)
        .ok_or_else(|| "TEST_GOVERNANCE_OBJECTS_REQUIRED".to_string())?;
    let now = Utc::now();
    let mut reports = Vec::new();
    for object in objects {
        let object_id = object
            .get("object_id")
            .and_then(Value::as_str)
            .unwrap_or("invalid-object");
        if object_filter.is_some_and(|filter| filter != object_id) {
            continue;
        }
        reports.push(test_governance_object_status(
            root,
            object,
            &candidate_commit,
            &manifest,
            now,
        ));
    }
    if let Some(filter) = object_filter {
        if reports.is_empty() {
            reports.push(serde_json::json!({
                "object_id": filter,
                "status": "not_selected"
            }));
        }
    }
    let blocked = reports
        .iter()
        .any(|report| report.get("status").and_then(Value::as_str) == Some("blocked"));
    let status = if blocked { "blocked" } else { "passed" };
    if strict && blocked {
        return Err(format!(
            "TEST_GOVERNANCE_BLOCKED:{}",
            reports
                .iter()
                .filter(|report| report.get("status").and_then(Value::as_str) == Some("blocked"))
                .filter_map(|report| report.get("object_id").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    Ok(serde_json::json!({
        "mode": "selected",
        "status": status,
        "objects": reports
    }))
}

pub(super) fn verify_test_admission_cli(
    args: &mut std::iter::Peekable<std::vec::IntoIter<String>>,
) {
    let root = project_root_or_cwd(args);
    let mut object_id: Option<String> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--object" | "-o" => {
                if object_id.is_some() {
                    fail("USAGE: appsdk verify --test-admission [project] [--object <id>]");
                }
                object_id = Some(args.next().unwrap_or_else(|| {
                    fail("USAGE: appsdk verify --test-admission [project] [--object <id>]")
                }));
            }
            _ => fail("USAGE: appsdk verify --test-admission [project] [--object <id>]"),
        }
    }
    verify_test_admission(&root, object_id.as_deref());
}

pub(super) fn verify_test_admission(root: &Path, object_id: Option<&str>) {
    assert_project_root_safe(root);
    let project = read_project(root);
    assert_declared_contracts(root, &project);
    assert_project_contract(root, &project);
    let report = test_governance_report(root, &project, object_id, false)
        .unwrap_or_else(|error| fail(error));
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    if report.get("status").and_then(Value::as_str) == Some("blocked") {
        fail("TEST_GOVERNANCE_BLOCKED");
    }
}
