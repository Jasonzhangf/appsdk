use super::{
    assert_id, assert_mutation_worktree, assert_no_symlink, compiled_relative, fail,
    guidance_contract, output, read_json, read_project, required_array, required_string,
    safe_relative,
};
use crate::guidance::ledger::atomic_write;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

fn validate_manifest(
    manifest: &Value,
    workflow_ids: &mut HashSet<String>,
    rule_ids: &mut HashSet<String>,
) {
    if manifest.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail(
            "GUIDANCE_MANIFEST_SCHEMA_INVALID",
            "use guidance manifest schema version 1",
        );
    }
    required_string(manifest, "skill_id", "GUIDANCE_MANIFEST_INVALID");
    required_string(manifest, "guidance_version", "GUIDANCE_MANIFEST_INVALID");
    let rules = required_array(manifest, "rules", "GUIDANCE_MANIFEST_INVALID");
    for rule in rules {
        let rule_id = required_string(rule, "rule_id", "GUIDANCE_RULE_INVALID");
        assert_id(rule_id, "GUIDANCE_RULE_ID_INVALID");
        if !rule_ids.insert(rule_id.to_string()) {
            fail(
                format!("GUIDANCE_DUPLICATE_RULE:{}", rule_id),
                "keep one canonical owner for each rule",
            );
        }
        if !matches!(
            rule.get("severity").and_then(Value::as_str),
            Some("advisory" | "warning" | "forbidden")
        ) || rule
            .get("description")
            .and_then(Value::as_str)
            .filter(|entry| !entry.is_empty())
            .is_none()
        {
            fail(
                "GUIDANCE_RULE_INVALID",
                "use advisory, warning, or forbidden",
            );
        }
        if rule.get("ontology_clause").is_some() {
            let clause_id = required_string(rule, "ontology_clause", "GUIDANCE_RULE_INVALID");
            assert_id(clause_id, "GUIDANCE_ONTOLOGY_CLAUSE_ID_INVALID");
        }
    }
    validate_ontology_clauses(manifest, rules);
    for workflow in required_array(manifest, "workflows", "GUIDANCE_MANIFEST_INVALID") {
        let domain = required_string(workflow, "domain", "GUIDANCE_WORKFLOW_INVALID");
        assert_id(domain, "GUIDANCE_DOMAIN_INVALID");
        if !workflow_ids.insert(domain.to_string()) {
            fail(
                format!("GUIDANCE_DUPLICATE_WORKFLOW:{}", domain),
                "keep one canonical owner for each workflow domain",
            );
        }
        required_string(workflow, "objective", "GUIDANCE_WORKFLOW_INVALID");
        let entry = required_string(workflow, "entry_node", "GUIDANCE_WORKFLOW_INVALID");
        let nodes = required_array(workflow, "nodes", "GUIDANCE_WORKFLOW_INVALID");
        if nodes.is_empty() {
            fail(
                "GUIDANCE_WORKFLOW_EMPTY",
                "declare at least one workflow node",
            );
        }
        let mut node_ids = HashSet::new();
        for node in nodes {
            let node_id = required_string(node, "node_id", "GUIDANCE_NODE_INVALID");
            assert_id(node_id, "GUIDANCE_NODE_ID_INVALID");
            if !node_ids.insert(node_id.to_string()) {
                fail(
                    format!("GUIDANCE_DUPLICATE_NODE:{}:{}", domain, node_id),
                    "keep one node definition per workflow",
                );
            }
            required_string(node, "instruction", "GUIDANCE_NODE_INVALID");
            required_string(node, "owner", "GUIDANCE_NODE_INVALID");
            let evidence = required_array(node, "evidence", "GUIDANCE_NODE_INVALID");
            if evidence
                .iter()
                .any(|item| item.as_str().filter(|entry| !entry.is_empty()).is_none())
            {
                fail(
                    "GUIDANCE_NODE_INVALID",
                    "use non-empty evidence identifiers",
                );
            }
            if let Some(reminders) = node.get("reminders") {
                for reminder in reminders.as_array().unwrap_or_else(|| {
                    fail(
                        "GUIDANCE_NODE_REMINDERS_INVALID",
                        "declare reminders as an array",
                    )
                }) {
                    required_string(reminder, "kind", "GUIDANCE_NODE_REMINDER_INVALID");
                    required_string(reminder, "command", "GUIDANCE_NODE_REMINDER_INVALID");
                    if reminder.get("blocking") != Some(&Value::Bool(false)) {
                        fail(
                            "GUIDANCE_NODE_REMINDER_BLOCKING",
                            "tour and memory reminders must remain non-blocking",
                        );
                    }
                }
            }
        }
        if !node_ids.contains(entry) {
            fail(
                format!("GUIDANCE_ENTRY_NODE_MISSING:{}:{}", domain, entry),
                "bind entry_node to a declared node",
            );
        }
        let mut edges = HashSet::new();
        for edge in required_array(workflow, "edges", "GUIDANCE_WORKFLOW_INVALID") {
            let from = required_string(edge, "from", "GUIDANCE_EDGE_INVALID");
            let to = required_string(edge, "to", "GUIDANCE_EDGE_INVALID");
            if !node_ids.contains(from) || !node_ids.contains(to) || from == to {
                fail(
                    format!("GUIDANCE_EDGE_INVALID:{}:{}:{}", domain, from, to),
                    "bind each edge to two different declared nodes",
                );
            }
            if !edges.insert((from.to_string(), to.to_string())) {
                fail(
                    format!("GUIDANCE_DUPLICATE_EDGE:{}:{}:{}", domain, from, to),
                    "keep one canonical edge",
                );
            }
        }
    }
}

fn validate_string_list(entry: &Value, field: &str, code: &str, require_non_empty: bool) {
    let values = required_array(entry, field, code);
    if require_non_empty && values.is_empty() {
        fail(code, "declare at least one entry");
    }
    if values
        .iter()
        .any(|value| value.as_str().filter(|text| !text.is_empty()).is_none())
    {
        fail(code, "use a non-empty string array");
    }
}

fn validate_constraints(entry: &Value, id: &str, code: &str) {
    let mut required = HashSet::new();
    for value in required_array(entry, "must_have", code) {
        let text = value
            .as_str()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| fail(code, "use non-empty constraint strings"));
        required.insert(text);
    }
    for value in required_array(entry, "must_not_have", code) {
        let text = value
            .as_str()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| fail(code, "use non-empty constraint strings"));
        if required.contains(text) {
            fail(
                format!("GUIDANCE_ONTOLOGY_CONSTRAINT_CONFLICT:{}:{}", id, text),
                "a constraint cannot be both required and forbidden",
            );
        }
    }
}

fn validate_ontology_models(
    ontology: &Value,
) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
    let mut object_ids = HashSet::new();
    for object in required_array(ontology, "objects", "GUIDANCE_ONTOLOGY_OBJECT_INVALID") {
        let id = required_string(object, "id", "GUIDANCE_ONTOLOGY_OBJECT_INVALID");
        assert_id(id, "GUIDANCE_ONTOLOGY_OBJECT_ID_INVALID");
        if !object_ids.insert(id.to_string()) {
            fail(
                format!("GUIDANCE_ONTOLOGY_OBJECT_DUPLICATE:{}", id),
                "keep one object definition per id",
            );
        }
        for field in ["name", "definition", "identity", "owner", "perception"] {
            required_string(object, field, "GUIDANCE_ONTOLOGY_OBJECT_INVALID");
        }
        validate_string_list(
            object,
            "attributes",
            "GUIDANCE_ONTOLOGY_OBJECT_INVALID",
            true,
        );
        validate_string_list(object, "source", "GUIDANCE_ONTOLOGY_OBJECT_INVALID", true);
        validate_constraints(object, id, "GUIDANCE_ONTOLOGY_OBJECT_INVALID");
    }

    let mut behavior_ids = HashSet::new();
    for behavior in required_array(ontology, "behaviors", "GUIDANCE_ONTOLOGY_BEHAVIOR_INVALID") {
        let id = required_string(behavior, "id", "GUIDANCE_ONTOLOGY_BEHAVIOR_INVALID");
        assert_id(id, "GUIDANCE_ONTOLOGY_BEHAVIOR_ID_INVALID");
        if !behavior_ids.insert(id.to_string()) {
            fail(
                format!("GUIDANCE_ONTOLOGY_BEHAVIOR_DUPLICATE:{}", id),
                "keep one behavior definition per id",
            );
        }
        for field in ["name", "owner", "perception"] {
            required_string(behavior, field, "GUIDANCE_ONTOLOGY_BEHAVIOR_INVALID");
        }
        for field in ["precondition", "input", "output", "effects", "source"] {
            validate_string_list(behavior, field, "GUIDANCE_ONTOLOGY_BEHAVIOR_INVALID", true);
        }
        let transition = behavior
            .get("transition")
            .filter(|value| value.is_object())
            .unwrap_or_else(|| {
                fail(
                    "GUIDANCE_ONTOLOGY_BEHAVIOR_TRANSITION_INVALID",
                    "declare the from and to states",
                )
            });
        required_string(
            transition,
            "from",
            "GUIDANCE_ONTOLOGY_BEHAVIOR_TRANSITION_INVALID",
        );
        required_string(
            transition,
            "to",
            "GUIDANCE_ONTOLOGY_BEHAVIOR_TRANSITION_INVALID",
        );
        validate_constraints(behavior, id, "GUIDANCE_ONTOLOGY_BEHAVIOR_INVALID");
    }

    let mut relation_ids = HashSet::new();
    for relation in required_array(ontology, "relations", "GUIDANCE_ONTOLOGY_RELATION_INVALID") {
        let id = required_string(relation, "id", "GUIDANCE_ONTOLOGY_RELATION_INVALID");
        assert_id(id, "GUIDANCE_ONTOLOGY_RELATION_ID_INVALID");
        if !relation_ids.insert(id.to_string()) {
            fail(
                format!("GUIDANCE_ONTOLOGY_RELATION_DUPLICATE:{}", id),
                "keep one relation definition per id",
            );
        }
        for field in ["name", "kind", "direction", "cardinality", "perception"] {
            required_string(relation, field, "GUIDANCE_ONTOLOGY_RELATION_INVALID");
        }
        validate_string_list(
            relation,
            "source",
            "GUIDANCE_ONTOLOGY_RELATION_INVALID",
            true,
        );
        validate_constraints(relation, id, "GUIDANCE_ONTOLOGY_RELATION_INVALID");
        for endpoint in [
            required_string(relation, "from", "GUIDANCE_ONTOLOGY_RELATION_INVALID"),
            required_string(relation, "to", "GUIDANCE_ONTOLOGY_RELATION_INVALID"),
        ] {
            if !object_ids.contains(endpoint) {
                fail(
                    format!(
                        "GUIDANCE_ONTOLOGY_RELATION_ENDPOINT_MISSING:{}:{}",
                        id, endpoint
                    ),
                    "declare both related objects",
                );
            }
        }
    }

    (object_ids, behavior_ids, relation_ids)
}

fn validate_ontology_clauses(manifest: &Value, rules: &[Value]) {
    let mut rule_clause = HashMap::new();
    let mut forbidden_rules = HashSet::new();
    for rule in rules {
        let rule_id = required_string(rule, "rule_id", "GUIDANCE_RULE_INVALID");
        if rule.get("severity").and_then(Value::as_str) == Some("forbidden") {
            forbidden_rules.insert(rule_id.to_string());
        }
        if let Some(clause_id) = rule.get("ontology_clause").and_then(Value::as_str) {
            rule_clause.insert(rule_id.to_string(), clause_id.to_string());
        }
    }

    let Some(ontology) = manifest.get("ontology") else {
        if rule_clause.is_empty() {
            return;
        }
        fail(
            "GUIDANCE_ONTOLOGY_CLAUSES_MISSING",
            "declare approved ontology clauses for rules that claim ontology governance",
        );
    };
    if ontology.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail(
            "GUIDANCE_ONTOLOGY_SCHEMA_INVALID",
            "use ontology clause schema version 1",
        );
    }
    let (object_ids, behavior_ids, relation_ids) = validate_ontology_models(ontology);
    let clauses = ontology
        .get("clauses")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            fail(
                "GUIDANCE_ONTOLOGY_CLAUSES_INVALID",
                "declare ontology clauses as an array",
            )
        });
    let mut clause_rule = HashMap::new();
    let mut clause_binding = HashMap::new();
    for clause in clauses {
        let clause_id = required_string(clause, "clause_id", "GUIDANCE_ONTOLOGY_CLAUSE_INVALID");
        assert_id(clause_id, "GUIDANCE_ONTOLOGY_CLAUSE_ID_INVALID");
        if clause_rule.contains_key(clause_id) {
            fail(
                format!("GUIDANCE_ONTOLOGY_CLAUSE_DUPLICATE:{}", clause_id),
                "keep one approved clause per rule",
            );
        }
        let rule_id = required_string(clause, "rule_id", "GUIDANCE_ONTOLOGY_CLAUSE_INVALID");
        if !forbidden_rules.contains(rule_id) {
            fail(
                format!("GUIDANCE_ONTOLOGY_RULE_MISSING:{}", rule_id),
                "bind each ontology clause to one declared forbidden rule",
            );
        }
        if clause_binding
            .insert(rule_id.to_string(), clause_id.to_string())
            .is_some()
        {
            fail(
                format!("GUIDANCE_ONTOLOGY_RULE_DUPLICATE_CLAUSE:{}", rule_id),
                "bind exactly one ontology clause to each rule",
            );
        }
        clause_rule.insert(clause_id.to_string(), rule_id.to_string());
        let axis = required_string(clause, "axis", "GUIDANCE_ONTOLOGY_CLAUSE_INVALID");
        let axis_ids = match axis {
            "object" => &object_ids,
            "behavior" => &behavior_ids,
            "relation" => &relation_ids,
            _ => fail(
                "GUIDANCE_ONTOLOGY_AXIS_INVALID",
                "use object, behavior, or relation",
            ),
        };
        let axis_id = required_string(clause, "axis_id", "GUIDANCE_ONTOLOGY_CLAUSE_INVALID");
        assert_id(axis_id, "GUIDANCE_ONTOLOGY_AXIS_ID_INVALID");
        if !axis_ids.contains(axis_id) {
            fail(
                format!(
                    "GUIDANCE_ONTOLOGY_AXIS_ENTRY_NOT_FOUND:{}:{}",
                    axis, axis_id
                ),
                "declare the referenced three-axis model entry",
            );
        }
        for field in [
            "scope",
            "scenario",
            "trigger",
            "state_before",
            "state_after",
            "owner",
        ] {
            required_string(clause, field, "GUIDANCE_ONTOLOGY_CLAUSE_INVALID");
        }
        for field in [
            "allowed_actions",
            "forbidden_actions",
            "effects",
            "evidence",
        ] {
            let entries = required_array(clause, field, "GUIDANCE_ONTOLOGY_CLAUSE_INVALID");
            if entries.is_empty()
                || entries
                    .iter()
                    .any(|entry| entry.as_str().filter(|value| !value.is_empty()).is_none())
            {
                fail(
                    format!("GUIDANCE_ONTOLOGY_FIELD_INVALID:{}:{}", clause_id, field),
                    "use a non-empty string array",
                );
            }
        }
        let approval = clause
            .get("approval")
            .filter(|value| value.is_object())
            .unwrap_or_else(|| {
                fail(
                    "GUIDANCE_ONTOLOGY_APPROVAL_MISSING",
                    "record exact user approval for the clause",
                )
            });
        if approval.get("state").and_then(Value::as_str) != Some("approved") {
            fail(
                format!("GUIDANCE_ONTOLOGY_CLAUSE_UNAPPROVED:{}", clause_id),
                "obtain exact user approval before binding the clause to a gate",
            );
        }
        for field in ["source", "text"] {
            required_string(approval, field, "GUIDANCE_ONTOLOGY_APPROVAL_INVALID");
        }
        let governance = clause
            .get("governance")
            .filter(|value| value.is_object())
            .unwrap_or_else(|| {
                fail(
                    format!("GUIDANCE_ONTOLOGY_CLAUSE_UNBOUND:{}", clause_id),
                    "bind the clause to one executable edge or gate",
                )
            });
        let gate_kind = required_string(governance, "gate_kind", "GUIDANCE_ONTOLOGY_GATE_INVALID");
        if !matches!(
            gate_kind,
            "identity"
                | "attribute"
                | "precondition"
                | "transition"
                | "relation"
                | "terminal"
                | "effect"
        ) {
            fail(
                "GUIDANCE_ONTOLOGY_GATE_INVALID",
                "use a declared ontology gate kind",
            );
        }
        for field in ["edge", "check", "allow_case", "deny_case", "observation"] {
            required_string(governance, field, "GUIDANCE_ONTOLOGY_GATE_INVALID");
        }
    }
    for (rule_id, clause_id) in &rule_clause {
        let bound_rule = clause_rule.get(clause_id.as_str()).unwrap_or_else(|| {
            fail(
                format!("GUIDANCE_ONTOLOGY_CLAUSE_NOT_FOUND:{}", clause_id),
                "declare the referenced ontology clause",
            );
        });
        if bound_rule != rule_id {
            fail(
                format!(
                    "GUIDANCE_ONTOLOGY_CLAUSE_RULE_MISMATCH:{}:{}:{}",
                    rule_id, clause_id, bound_rule
                ),
                "bind each ontology clause and rule pair in both directions",
            );
        }
    }
    for (clause_id, rule_id) in &clause_rule {
        if rule_clause.get(rule_id.as_str()) != Some(clause_id) {
            fail(
                format!(
                    "GUIDANCE_ONTOLOGY_CLAUSE_UNREFERENCED:{}:{}",
                    clause_id, rule_id
                ),
                "reference each ontology clause from the rule it binds",
            );
        }
    }
}

pub(super) fn compile(root: &Path) {
    assert_mutation_worktree(root);
    let project = read_project(root);
    let guidance = guidance_contract(&project).unwrap_or_else(|| {
        fail(
            "GUIDANCE_NOT_CONFIGURED",
            "declare guidance rule_sources in .appsdk/project.json",
        )
    });
    let enforcement = guidance
        .get("enforcement")
        .and_then(Value::as_str)
        .unwrap_or("advisory");
    if !matches!(enforcement, "advisory" | "warning" | "forbidden") {
        fail(
            "GUIDANCE_ENFORCEMENT_INVALID",
            "use advisory, warning, or forbidden",
        );
    }
    let mut sources = required_array(guidance, "rule_sources", "GUIDANCE_SOURCES_INVALID").clone();
    sources.sort_by(|left, right| {
        left.get("precedence")
            .and_then(Value::as_u64)
            .cmp(&right.get("precedence").and_then(Value::as_u64))
            .then_with(|| {
                left.get("source_id")
                    .and_then(Value::as_str)
                    .cmp(&right.get("source_id").and_then(Value::as_str))
            })
    });
    let mut source_ids = HashSet::new();
    let mut precedences = HashSet::new();
    let mut compiled_sources = Vec::new();
    let mut workflows = Vec::new();
    let mut rules = Vec::new();
    let mut workflow_ids = HashSet::new();
    let mut rule_ids = HashSet::new();
    for source in sources {
        let source_id = required_string(&source, "source_id", "GUIDANCE_SOURCE_INVALID");
        assert_id(source_id, "GUIDANCE_SOURCE_ID_INVALID");
        let precedence = source
            .get("precedence")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| {
                fail(
                    "GUIDANCE_SOURCE_PRECEDENCE_INVALID",
                    "declare one numeric precedence per source",
                )
            });
        if !source_ids.insert(source_id.to_string()) || !precedences.insert(precedence) {
            fail(
                "GUIDANCE_SOURCE_ORDER_AMBIGUOUS",
                "use unique source IDs and precedence values",
            );
        }
        let kind = required_string(&source, "kind", "GUIDANCE_SOURCE_INVALID");
        if !matches!(kind, "agents" | "skill") {
            fail(
                "GUIDANCE_SOURCE_KIND_INVALID",
                "use an agents or skill source",
            );
        }
        let relative_text = required_string(&source, "path", "GUIDANCE_SOURCE_INVALID");
        let relative = safe_relative(relative_text, "GUIDANCE_RULE_SOURCE_PATH_ESCAPE");
        assert_no_symlink(root, &relative, "GUIDANCE_RULE_SOURCE_SYMLINK");
        let required = source
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let path = root.join(&relative);
        let bytes = fs::read(&path).ok();
        if required && bytes.is_none() {
            fail(
                format!("GUIDANCE_REQUIRED_SOURCE_MISSING:{}", source_id),
                "restore the declared source or update the project contract",
            );
        }
        let mut compiled_source = serde_json::json!({
            "source_id": source_id,
            "kind": kind,
            "path": relative_text,
            "required": required,
            "precedence": precedence,
            "present": bytes.is_some()
        });
        if let Some(contract_text) = source.get("contract_path").and_then(Value::as_str) {
            let contract_relative =
                safe_relative(contract_text, "GUIDANCE_RULE_SOURCE_PATH_ESCAPE");
            assert_no_symlink(root, &contract_relative, "GUIDANCE_RULE_SOURCE_SYMLINK");
            let contract_path = root.join(&contract_relative);
            let contract_bytes = fs::read(&contract_path).unwrap_or_else(|_| {
                fail(
                    format!("GUIDANCE_CONTRACT_MISSING:{}", source_id),
                    "restore the declared machine guidance contract",
                )
            });
            let manifest: Value = serde_json::from_slice(&contract_bytes).unwrap_or_else(|_| {
                fail(
                    format!("GUIDANCE_CONTRACT_INVALID:{}", source_id),
                    "repair the declared machine guidance contract",
                )
            });
            validate_manifest(&manifest, &mut workflow_ids, &mut rule_ids);
            rules.extend(required_array(&manifest, "rules", "GUIDANCE_MANIFEST_INVALID").clone());
            workflows.extend(
                required_array(&manifest, "workflows", "GUIDANCE_MANIFEST_INVALID").clone(),
            );
            compiled_source["contract_path"] = Value::String(contract_text.to_string());
        }
        compiled_sources.push(compiled_source);
    }
    if workflows.is_empty() {
        fail(
            "GUIDANCE_WORKFLOWS_MISSING",
            "declare at least one Skill machine contract",
        );
    }
    let compiled = serde_json::json!({
        "schema_version": 1,
        "harness": "appsdk-development-process-control",
        "enforcement": enforcement,
        "sources": compiled_sources,
        "rules": rules,
        "workflows": workflows
    });
    let relative = compiled_relative(&project);
    assert_no_symlink(root, &relative, "GUIDANCE_COMPILED_SYMLINK");
    atomic_write(&root.join(relative), &compiled);
    output(&compiled);
}

pub(super) fn load_compiled(root: &Path, project: &Value) -> Option<Value> {
    let relative = compiled_relative(project);
    assert_no_symlink(root, &relative, "GUIDANCE_COMPILED_SYMLINK");
    let path = root.join(relative);
    if !path.is_file() {
        return None;
    }
    let mut value = read_json(&path, "GUIDANCE_COMPILED_INVALID");
    if value.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail("GUIDANCE_COMPILED_INVALID", "rerun appsdk guide compile");
    }
    for field in ["sources", "rules", "workflows"] {
        required_array(&value, field, "GUIDANCE_COMPILED_INVALID");
    }
    // Retired identity fields from older artifacts are not projected to consumers.
    let object = value.as_object_mut().unwrap();
    object.remove("manifest_hash");
    object.remove("project_contract_digest");
    for source in object["sources"].as_array_mut().unwrap() {
        if let Some(source) = source.as_object_mut() {
            source.remove("digest");
            source.remove("contract_digest");
        }
    }
    Some(value)
}
