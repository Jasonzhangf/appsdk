use super::{
    assert_id, assert_mutation_worktree, assert_no_symlink, compiled_relative, digest_bytes,
    digest_value, fail, guidance_contract, output, read_json, read_project, required_array,
    required_string, safe_relative,
};
use crate::guidance::ledger::atomic_write;
use serde_json::Value;
use std::collections::HashSet;
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
    for rule in required_array(manifest, "rules", "GUIDANCE_MANIFEST_INVALID") {
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
    }
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
            "present": bytes.is_some(),
            "digest": bytes.as_ref().map(|content| digest_bytes(content))
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
            compiled_source["contract_digest"] = Value::String(digest_bytes(&contract_bytes));
        }
        compiled_sources.push(compiled_source);
    }
    if workflows.is_empty() {
        fail(
            "GUIDANCE_WORKFLOWS_MISSING",
            "declare at least one Skill machine contract",
        );
    }
    let mut compiled = serde_json::json!({
        "schema_version": 1,
        "harness": "appsdk-development-process-control",
        "enforcement": enforcement,
        "project_contract_digest": digest_value(&project),
        "sources": compiled_sources,
        "rules": rules,
        "workflows": workflows
    });
    let hash = digest_value(&compiled);
    compiled["manifest_hash"] = Value::String(hash);
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
    let recorded = value
        .get("manifest_hash")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            fail(
                "GUIDANCE_COMPILED_HASH_MISSING",
                "rerun appsdk guide compile",
            )
        })
        .to_string();
    value.as_object_mut().unwrap().remove("manifest_hash");
    if digest_value(&value) != recorded {
        fail(
            "GUIDANCE_COMPILED_HASH_MISMATCH",
            "rerun appsdk guide compile",
        );
    }
    value["manifest_hash"] = Value::String(recorded);
    Some(value)
}

pub(super) fn current_rule_sources(root: &Path, compiled: &Value) -> Value {
    let sources = required_array(compiled, "sources", "GUIDANCE_COMPILED_INVALID");
    Value::Array(
        sources
            .iter()
            .map(|source| {
                let mut current = source.clone();
                let path_text = required_string(source, "path", "GUIDANCE_COMPILED_INVALID");
                let relative = safe_relative(path_text, "GUIDANCE_RULE_SOURCE_PATH_ESCAPE");
                assert_no_symlink(root, &relative, "GUIDANCE_RULE_SOURCE_SYMLINK");
                let bytes = fs::read(root.join(relative)).ok();
                current["present"] = Value::Bool(bytes.is_some());
                current["digest"] = bytes
                    .as_ref()
                    .map(|bytes| Value::String(digest_bytes(bytes)))
                    .unwrap_or(Value::Null);
                if let Some(contract_text) = source.get("contract_path").and_then(Value::as_str) {
                    let contract_relative =
                        safe_relative(contract_text, "GUIDANCE_RULE_SOURCE_PATH_ESCAPE");
                    assert_no_symlink(root, &contract_relative, "GUIDANCE_RULE_SOURCE_SYMLINK");
                    current["contract_digest"] = fs::read(root.join(contract_relative))
                        .ok()
                        .map(|bytes| Value::String(digest_bytes(&bytes)))
                        .unwrap_or(Value::Null);
                }
                current
            })
            .collect(),
    )
}

pub(super) fn compiled_context_drift(
    root: &Path,
    project: &Value,
    compiled: &Value,
) -> Option<&'static str> {
    if compiled
        .get("project_contract_digest")
        .and_then(Value::as_str)
        != Some(digest_value(project).as_str())
    {
        return Some("project_contract");
    }
    if compiled.get("sources") != Some(&current_rule_sources(root, compiled)) {
        return Some("rule_sources");
    }
    None
}
