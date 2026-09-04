use chrono::Utc;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

mod compiler;
mod ledger;
mod projector;

use compiler::{compile, compiled_context_drift, current_rule_sources, load_compiled};
use ledger::{atomic_write, event_file, plan_file, read_events, read_plan};
use projector::{close, next_step, project_status, step_events};

const DOMAINS: [&str; 11] = [
    "bootstrap",
    "migration",
    "governance-preflight",
    "develop",
    "debug",
    "review",
    "delivery",
    "integration",
    "promotion",
    "freeze",
    "cleanup",
];

const REVISION_REASONS: [&str; 13] = [
    "new_evidence",
    "new_blocker",
    "hypothesis_rejected",
    "scope_changed",
    "owner_changed",
    "source_drift",
    "environment_changed",
    "prior_solution_matched",
    "rule_context_changed",
    "guidance_manifest_changed",
    "project_contract_changed",
    "gate_contract_changed",
    "dependency_input_changed",
];

fn fail(code: impl AsRef<str>, next: &str) -> ! {
    eprintln!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "error": code.as_ref(),
            "retry_allowed": false,
            "preserved_state": true,
            "next": next
        }))
        .unwrap()
    );
    std::process::exit(1);
}

fn output(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap());
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

fn digest_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn digest_value(value: &Value) -> String {
    digest_bytes(canonical(value).as_bytes())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn assert_id(value: &str, code: &str) {
    if !valid_id(value) {
        fail(
            code,
            "use a non-empty alphanumeric, dash, underscore, or dot identifier",
        );
    }
}

fn safe_relative(value: &str, code: &str) -> PathBuf {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir
                    | Component::CurDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        fail(code, "declare a normalized project-relative path");
    }
    path.to_path_buf()
}

fn assert_no_symlink(root: &Path, relative: &Path, code: &str) {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        if fs::symlink_metadata(&current)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            fail(
                code,
                "replace the declared symlink with a project-owned regular path",
            );
        }
    }
}

fn read_json(path: &Path, code: &str) -> Value {
    serde_json::from_str(
        &fs::read_to_string(path)
            .unwrap_or_else(|_| fail(code, "restore the declared JSON file and rerun once")),
    )
    .unwrap_or_else(|_| fail(code, "repair the declared JSON and rerun once"))
}

fn read_project(root: &Path) -> Value {
    let relative = Path::new(".appsdk/project.json");
    assert_no_symlink(root, relative, "GUIDANCE_PROJECT_SYMLINK");
    read_json(&root.join(relative), "GUIDANCE_PROJECT_MISSING_OR_INVALID")
}

fn read_goal(root: &Path) -> Value {
    let relative = Path::new(".appsdk/goal.json");
    assert_no_symlink(root, relative, "GUIDANCE_GOAL_SYMLINK");
    read_json(&root.join(relative), "GUIDANCE_GOAL_MISSING_OR_INVALID")
}

fn append_event(path: &Path, value: &Value) {
    ledger::append(path, value);
}

fn guidance_contract(project: &Value) -> Option<&Value> {
    project.get("guidance")
}

fn compiled_relative(project: &Value) -> PathBuf {
    let value = guidance_contract(project)
        .and_then(|guidance| guidance.get("compiled_manifest"))
        .and_then(Value::as_str)
        .unwrap_or(".appsdk/guidance/compiled.json");
    safe_relative(value, "GUIDANCE_COMPILED_PATH_ESCAPE")
}

fn assert_mutation_worktree(root: &Path) {
    let result = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "symbolic-ref",
            "--quiet",
            "--short",
            "HEAD",
        ])
        .output();
    if let Ok(result) = result {
        if result.status.success() {
            let branch = String::from_utf8_lossy(&result.stdout);
            if matches!(branch.trim(), "main" | "master") {
                fail(
                    "GUIDANCE_MAIN_MUTATION_FORBIDDEN",
                    "create a clean owner worktree from latest origin/main",
                );
            }
        }
    }
}

fn assert_owner_worktree(root: &Path) {
    let result = Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap_or("."),
            "symbolic-ref",
            "--quiet",
            "--short",
            "HEAD",
        ])
        .output()
        .unwrap_or_else(|_| {
            fail(
                "GUIDANCE_OWNER_WORKTREE_REQUIRED",
                "create a named clean owner worktree from latest origin/main",
            )
        });
    if !result.status.success() {
        fail(
            "GUIDANCE_OWNER_WORKTREE_REQUIRED",
            "create a named clean owner worktree from latest origin/main",
        );
    }
    if matches!(
        String::from_utf8_lossy(&result.stdout).trim(),
        "main" | "master"
    ) {
        fail(
            "GUIDANCE_MAIN_MUTATION_FORBIDDEN",
            "create a named clean owner worktree from latest origin/main",
        );
    }
}

fn required_string<'a>(value: &'a Value, key: &str, code: &str) -> &'a str {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|entry| !entry.is_empty())
        .unwrap_or_else(|| fail(code, "repair the declared guidance contract"))
}

fn required_array<'a>(value: &'a Value, key: &str, code: &str) -> &'a Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail(code, "repair the declared guidance contract"))
}

fn module<'a>(project: &'a Value, requested: Option<&str>) -> &'a Value {
    let modules = project
        .get("modules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("GUIDANCE_MODULES_INVALID", "repair .appsdk/project.json"));
    if let Some(requested) = requested {
        return modules
            .iter()
            .find(|module| module.get("module_id").and_then(Value::as_str) == Some(requested))
            .unwrap_or_else(|| {
                fail(
                    format!("GUIDANCE_MODULE_NOT_FOUND:{}", requested),
                    "select a declared project module",
                )
            });
    }
    modules
        .first()
        .unwrap_or_else(|| fail("GUIDANCE_MODULES_EMPTY", "declare a project module"))
}

fn workflow<'a>(compiled: &'a Value, domain: &str) -> &'a Value {
    compiled
        .get("workflows")
        .and_then(Value::as_array)
        .and_then(|workflows| {
            workflows
                .iter()
                .find(|workflow| workflow.get("domain").and_then(Value::as_str) == Some(domain))
        })
        .unwrap_or_else(|| {
            fail(
                format!("GUIDANCE_DOMAIN_NOT_FOUND:{}", domain),
                "select a domain declared by the compiled guidance manifest",
            )
        })
}

fn git(root: &Path, args: &[&str], code: &str) -> String {
    let result = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap_or_else(|_| fail(code, "run guidance in a Git owner worktree"));
    if !result.status.success() {
        fail(code, "run guidance in a Git owner worktree");
    }
    String::from_utf8_lossy(&result.stdout).trim().to_string()
}

fn scope_state(root: &Path, paths: &[String]) -> String {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain=v1", "--untracked-files=all", "--"]);
    for path in paths {
        command.arg(path);
    }
    let result = command.output().unwrap_or_else(|_| {
        fail(
            "GUIDANCE_SOURCE_GIT_UNAVAILABLE",
            "run guidance in a Git owner worktree",
        )
    });
    if !result.status.success() {
        fail(
            "GUIDANCE_SOURCE_GIT_UNAVAILABLE",
            "run guidance in a Git owner worktree",
        );
    }
    digest_bytes(&result.stdout)
}

fn rule_context(
    root: &Path,
    project: &Value,
    goal: &Value,
    compiled: &Value,
    module: &Value,
    scope_paths: &[String],
) -> Value {
    let commit = git(
        root,
        &["rev-parse", "HEAD"],
        "GUIDANCE_SOURCE_GIT_UNAVAILABLE",
    );
    let tree = git(
        root,
        &["rev-parse", "HEAD^{tree}"],
        "GUIDANCE_SOURCE_GIT_UNAVAILABLE",
    );
    let mut sorted_scope = scope_paths.to_vec();
    sorted_scope.sort();
    sorted_scope.dedup();
    serde_json::json!({
        "project_contract_hash": digest_value(project),
        "goal_hash": digest_value(goal),
        "guidance_manifest_hash": required_string(compiled, "manifest_hash", "GUIDANCE_COMPILED_INVALID"),
        "rule_sources": current_rule_sources(root, compiled),
        "source_commit": commit,
        "source_tree_hash": tree,
        "scope_hash": digest_value(&serde_json::json!(sorted_scope)),
        "scope_state_hash": scope_state(root, scope_paths),
        "owner": required_string(module, "source_owner", "GUIDANCE_MODULE_OWNER_INVALID")
    })
}

fn normalize_surface(value: &str) -> &str {
    value
        .strip_suffix("/**")
        .or_else(|| value.strip_suffix("/*"))
        .unwrap_or(value)
        .trim_end_matches('/')
}

fn scope_allowed(module: &Value, candidate: &str) -> bool {
    let candidate = normalize_surface(candidate);
    ["owned_paths", "contract_paths"]
        .iter()
        .filter_map(|key| module.get(*key).and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .map(normalize_surface)
        .any(|allowed| {
            allowed == "."
                || candidate == allowed
                || candidate.starts_with(&format!("{}/", allowed))
        })
}

fn plan(root: &Path, task: &str, input: &str) {
    assert_owner_worktree(root);
    let project = read_project(root);
    let compiled = load_compiled(root, &project).unwrap_or_else(|| {
        fail(
            "GUIDANCE_NOT_COMPILED",
            "run appsdk guide compile <project>",
        )
    });
    if let Some(reason) = compiled_context_drift(root, &project, &compiled) {
        fail(
            format!("GUIDANCE_COMPILED_CONTEXT_DRIFT:{}", reason),
            "run appsdk guide compile <project>",
        );
    }
    let input_relative = safe_relative(input, "GUIDANCE_INPUT_PATH_ESCAPE");
    assert_no_symlink(root, &input_relative, "GUIDANCE_INPUT_SYMLINK");
    let proposal = read_json(&root.join(input_relative), "GUIDANCE_PLAN_INPUT_INVALID");
    for field in [
        "current_node",
        "next_transition",
        "rule_context",
        "source_commit",
        "source_tree_hash",
        "scope_hash",
    ] {
        if proposal.get(field).is_some() {
            fail(
                format!("GUIDANCE_DERIVED_FIELD_FORBIDDEN:{}", field),
                "remove derived fields and resubmit the agent-authored plan",
            );
        }
    }
    if proposal.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail(
            "GUIDANCE_PLAN_SCHEMA_INVALID",
            "submit plan schema version 1",
        );
    }
    let proposal_task = required_string(&proposal, "task_id", "GUIDANCE_PLAN_INVALID");
    if proposal_task != task {
        fail(
            "GUIDANCE_TASK_MISMATCH",
            "use the same task ID in the command and plan",
        );
    }
    let mode = required_string(&proposal, "mode", "GUIDANCE_PLAN_INVALID");
    let selected_workflow = workflow(&compiled, mode);
    let goal = read_goal(root);
    if required_string(&proposal, "goal_id", "GUIDANCE_PLAN_INVALID")
        != required_string(&goal, "goal_id", "GUIDANCE_GOAL_INVALID")
    {
        fail(
            "GUIDANCE_GOAL_MISMATCH",
            "bind the plan to the current project goal",
        );
    }
    let module_id = required_string(&proposal, "module_id", "GUIDANCE_PLAN_INVALID");
    let selected_module = module(&project, Some(module_id));
    let owner = required_string(
        selected_module,
        "source_owner",
        "GUIDANCE_MODULE_OWNER_INVALID",
    );
    let scope_values = required_array(&proposal, "scope_paths", "GUIDANCE_PLAN_INVALID");
    if scope_values.is_empty() {
        fail(
            "GUIDANCE_SCOPE_EMPTY",
            "declare at least one module-owned scope path",
        );
    }
    let mut scope_paths = Vec::new();
    for value in scope_values {
        let value = value
            .as_str()
            .filter(|entry| !entry.is_empty())
            .unwrap_or_else(|| {
                fail(
                    "GUIDANCE_SCOPE_INVALID",
                    "use non-empty project-relative scope paths",
                )
            });
        safe_relative(value, "GUIDANCE_SCOPE_PATH_ESCAPE");
        if !scope_allowed(selected_module, value) {
            fail(
                format!("GUIDANCE_SCOPE_NOT_OWNED:{}", value),
                "select a path owned by the declared module",
            );
        }
        scope_paths.push(value.to_string());
    }
    let steps = required_array(&proposal, "steps", "GUIDANCE_PLAN_INVALID");
    if steps.is_empty() {
        fail("GUIDANCE_PLAN_EMPTY", "declare at least one adjacent step");
    }
    let nodes = required_array(selected_workflow, "nodes", "GUIDANCE_WORKFLOW_INVALID");
    let edges = required_array(selected_workflow, "edges", "GUIDANCE_WORKFLOW_INVALID");
    let entry = required_string(selected_workflow, "entry_node", "GUIDANCE_WORKFLOW_INVALID");
    let mut step_ids = HashSet::new();
    let mut previous: Option<&str> = None;
    for step in steps {
        let step_id = required_string(step, "step_id", "GUIDANCE_PLAN_STEP_INVALID");
        assert_id(step_id, "GUIDANCE_PLAN_STEP_ID_INVALID");
        if !step_ids.insert(step_id.to_string()) {
            fail(
                format!("GUIDANCE_DUPLICATE_STEP:{}", step_id),
                "use one identifier per plan step",
            );
        }
        let node_id = required_string(step, "node_id", "GUIDANCE_PLAN_STEP_INVALID");
        let node = nodes
            .iter()
            .find(|node| node.get("node_id").and_then(Value::as_str) == Some(node_id))
            .unwrap_or_else(|| {
                fail(
                    format!("GUIDANCE_UNKNOWN_NODE:{}:{}", mode, node_id),
                    "select a node declared by the workflow",
                )
            });
        if previous.is_none() && node_id != entry {
            fail(
                format!("GUIDANCE_PLAN_ENTRY_MISMATCH:{}:{}", entry, node_id),
                "start from the projected workflow entry node",
            );
        }
        if let Some(from) = previous {
            let adjacent = edges.iter().any(|edge| {
                edge.get("from").and_then(Value::as_str) == Some(from)
                    && edge.get("to").and_then(Value::as_str) == Some(node_id)
            });
            if !adjacent {
                fail(
                    format!("GUIDANCE_NON_ADJACENT_TRANSITION:{}:{}", from, node_id),
                    "regenerate the plan using only declared adjacent edges",
                );
            }
        }
        required_string(step, "action", "GUIDANCE_PLAN_STEP_INVALID");
        let expected_owner = match required_string(node, "owner", "GUIDANCE_NODE_INVALID") {
            "project" => owner,
            other => other,
        };
        if step.get("owner").and_then(Value::as_str) != Some(expected_owner) {
            fail(
                format!("GUIDANCE_OWNER_MISMATCH:{}:{}", step_id, expected_owner),
                "bind the step to its canonical owner",
            );
        }
        let expected = required_array(step, "expected_evidence", "GUIDANCE_PLAN_STEP_INVALID");
        let required = required_array(node, "evidence", "GUIDANCE_NODE_INVALID");
        if required.iter().any(|required| !expected.contains(required)) {
            fail(
                format!("GUIDANCE_EXPECTED_EVIDENCE_MISSING:{}", step_id),
                "include every evidence type required by the workflow node",
            );
        }
        previous = Some(node_id);
    }
    let context = rule_context(
        root,
        &project,
        &goal,
        &compiled,
        selected_module,
        &scope_paths,
    );
    let existing_path = plan_file(root, task);
    let existing = existing_path.is_file().then(|| read_plan(root, task));
    let revision_reason = proposal.get("revision_reason").and_then(Value::as_str);
    if existing.is_some()
        && !revision_reason.is_some_and(|reason| REVISION_REASONS.contains(&reason))
    {
        fail(
            "GUIDANCE_PLAN_REVISION_REASON_REQUIRED",
            "resubmit with one declared revision_reason",
        );
    }
    if existing.is_none() && revision_reason.is_some() {
        fail(
            "GUIDANCE_INITIAL_PLAN_HAS_REVISION_REASON",
            "remove revision_reason from the first PlanRecord",
        );
    }
    let sequence = read_events(root, task)
        .iter()
        .filter(|event| event.get("record_type").and_then(Value::as_str) == Some("PlanRecord"))
        .count()
        + 1;
    let created_at = Utc::now().to_rfc3339();
    let mut record = serde_json::json!({
        "schema_version": 1,
        "record_type": "PlanRecord",
        "plan_id": format!("plan-{}-{}", task, sequence),
        "task_id": task,
        "goal_id": required_string(&proposal, "goal_id", "GUIDANCE_PLAN_INVALID"),
        "module_id": module_id,
        "mode": mode,
        "objective": required_string(&proposal, "objective", "GUIDANCE_PLAN_INVALID"),
        "scope_paths": scope_paths,
        "steps": steps,
        "rule_context": context,
        "created_at": created_at
    });
    let hash = digest_value(&record);
    record["plan_hash"] = Value::String(hash.clone());
    if let Some(existing) = existing {
        let previous_hash = required_string(&existing, "plan_hash", "GUIDANCE_PLAN_INVALID");
        let revision = serde_json::json!({
            "schema_version": 1,
            "record_type": "PlanRevisionRecord",
            "revision_id": format!("revision-{}-{}", task, sequence),
            "task_id": task,
            "previous_plan_hash": previous_hash,
            "new_plan_hash": hash,
            "reason": revision_reason.unwrap(),
            "created_at": Utc::now().to_rfc3339()
        });
        append_event(&event_file(root, task), &revision);
    }
    append_event(&event_file(root, task), &record);
    atomic_write(&existing_path, &record);
    output(&serde_json::json!({
        "accepted": true,
        "plan_id": record["plan_id"],
        "plan_hash": record["plan_hash"],
        "next": {"step_id": steps[0]["step_id"], "node_id": steps[0]["node_id"]}
    }));
}

fn context_drift(
    root: &Path,
    project: &Value,
    compiled: &Value,
    plan: &Value,
) -> Option<&'static str> {
    let goal = read_goal(root);
    let module_id = required_string(plan, "module_id", "GUIDANCE_PLAN_INVALID");
    let selected_module = module(project, Some(module_id));
    let scope_paths = required_array(plan, "scope_paths", "GUIDANCE_PLAN_INVALID")
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| fail("GUIDANCE_PLAN_INVALID", "revise the active plan"))
                .to_string()
        })
        .collect::<Vec<_>>();
    let current = rule_context(
        root,
        project,
        &goal,
        compiled,
        selected_module,
        &scope_paths,
    );
    let recorded = plan
        .get("rule_context")
        .unwrap_or_else(|| fail("GUIDANCE_PLAN_INVALID", "revise the active plan"));
    for (key, reason) in [
        ("project_contract_hash", "project_contract"),
        ("goal_hash", "goal"),
        ("guidance_manifest_hash", "guidance_manifest"),
        ("rule_sources", "rule_context"),
        ("owner", "owner"),
        ("scope_hash", "scope"),
        ("source_commit", "source"),
        ("source_tree_hash", "source"),
        ("scope_state_hash", "source"),
    ] {
        if current.get(key) != recorded.get(key) {
            return Some(reason);
        }
    }
    None
}

fn lifecycle_projection(project: &Value, selected_module: &Value) -> Value {
    serde_json::json!({
        "project_stage": project.pointer("/lifecycle/stage").and_then(Value::as_str),
        "module_id": selected_module.get("module_id").and_then(Value::as_str),
        "module_stage": selected_module.get("stage").and_then(Value::as_str)
    })
}

fn update(root: &Path, task: &str, input: &str) {
    assert_owner_worktree(root);
    let project = read_project(root);
    let compiled = load_compiled(root, &project).unwrap_or_else(|| {
        fail(
            "GUIDANCE_NOT_COMPILED",
            "run appsdk guide compile <project>",
        )
    });
    let plan = read_plan(root, task);
    let input_relative = safe_relative(input, "GUIDANCE_INPUT_PATH_ESCAPE");
    assert_no_symlink(root, &input_relative, "GUIDANCE_INPUT_SYMLINK");
    let result = read_json(&root.join(input_relative), "GUIDANCE_UPDATE_INPUT_INVALID");
    if result.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail(
            "GUIDANCE_UPDATE_SCHEMA_INVALID",
            "submit step result schema version 1",
        );
    }
    for field in ["scope_paths", "owner", "current_node", "next_transition"] {
        if result.get(field).is_some() {
            fail(
                format!("GUIDANCE_PLAN_REVISION_REQUIRED:{}", field),
                "revise the plan before changing bound context",
            );
        }
    }
    let event_id = required_string(&result, "event_id", "GUIDANCE_UPDATE_INVALID");
    assert_id(event_id, "GUIDANCE_EVENT_ID_INVALID");
    let input_hash = digest_value(&result);
    let events = read_events(root, task);
    if let Some(existing) = events.iter().find(|event| {
        event.get("record_type").and_then(Value::as_str) == Some("StepExecutionRecord")
            && event.get("event_id").and_then(Value::as_str) == Some(event_id)
    }) {
        if existing.get("input_hash").and_then(Value::as_str) == Some(&input_hash) {
            output(
                &serde_json::json!({"accepted": true, "idempotent": true, "event_id": event_id}),
            );
            return;
        }
        fail(
            format!("GUIDANCE_EVENT_CONFLICT:{}", event_id),
            "use a new event ID for different content",
        );
    }
    if let Some(reason) = context_drift(root, &project, &compiled, &plan) {
        fail(
            format!("GUIDANCE_CONTEXT_DRIFT:{}", reason),
            "submit a PlanRevisionRecord with the matching drift reason",
        );
    }
    let plan_hash = required_string(&plan, "plan_hash", "GUIDANCE_PLAN_INVALID");
    let current_events = step_events(&events, plan_hash);
    let next = match next_step(&plan, &current_events) {
        Ok(Some(step)) => step,
        Ok(None) => fail(
            "GUIDANCE_WORKFLOW_COMPLETE",
            "run appsdk guide close <project> --task <id>",
        ),
        Err(_) => fail(
            "GUIDANCE_PLAN_BLOCKED",
            "submit a PlanRevisionRecord with reason new_blocker",
        ),
    };
    let step_id = required_string(&result, "step_id", "GUIDANCE_UPDATE_INVALID");
    if next.get("step_id").and_then(Value::as_str) != Some(step_id) {
        fail(
            format!("GUIDANCE_STEP_NOT_CURRENT:{}", step_id),
            "update only the projected next step",
        );
    }
    let outcome = required_string(&result, "result", "GUIDANCE_UPDATE_INVALID");
    if !matches!(outcome, "pass" | "fail" | "blocked") {
        fail("GUIDANCE_STEP_RESULT_INVALID", "use pass, fail, or blocked");
    }
    let evidence = required_array(&result, "evidence", "GUIDANCE_UPDATE_INVALID");
    let observations = required_array(&result, "observations", "GUIDANCE_UPDATE_INVALID");
    if evidence
        .iter()
        .any(|value| value.as_str().filter(|entry| !entry.is_empty()).is_none())
        || observations.iter().any(|value| value.as_str().is_none())
    {
        fail(
            "GUIDANCE_UPDATE_INVALID",
            "use string observations and non-empty evidence IDs",
        );
    }
    if outcome == "pass"
        && next
            .get("expected_evidence")
            .and_then(Value::as_array)
            .is_some_and(|required| !required.is_empty())
        && evidence.is_empty()
    {
        fail(
            format!("GUIDANCE_PASS_REQUIRES_EVIDENCE:{}", step_id),
            "attach real evidence before reporting pass",
        );
    }
    let event = serde_json::json!({
        "schema_version": 1,
        "record_type": "StepExecutionRecord",
        "event_id": event_id,
        "task_id": task,
        "step_id": step_id,
        "result": outcome,
        "observations": observations,
        "evidence": evidence,
        "plan_hash": plan_hash,
        "input_hash": input_hash,
        "created_at": Utc::now().to_rfc3339()
    });
    append_event(&event_file(root, task), &event);
    let status = project_status(root, None, Some(task), None, false);
    output(&serde_json::json!({
        "accepted": true,
        "idempotent": false,
        "event_id": event_id,
        "status": status
    }));
}

fn parse_options(args: &mut impl Iterator<Item = String>) -> (Option<String>, Option<String>) {
    let mut task = None;
    let mut module = None;
    while let Some(option) = args.next() {
        match option.as_str() {
            "--task" => {
                task =
                    Some(args.next().unwrap_or_else(|| {
                        fail("GUIDANCE_USAGE", "provide a task ID after --task")
                    }));
            }
            "--module" => {
                module = Some(args.next().unwrap_or_else(|| {
                    fail("GUIDANCE_USAGE", "provide a module ID after --module")
                }));
            }
            _ => fail("GUIDANCE_USAGE", "use only --task or --module"),
        }
    }
    (task, module)
}

fn parse_mutation_options(args: &mut impl Iterator<Item = String>) -> (String, String) {
    let mut task = None;
    let mut input = None;
    while let Some(option) = args.next() {
        match option.as_str() {
            "--task" => task = args.next(),
            "--input" => input = args.next(),
            _ => fail("GUIDANCE_USAGE", "use --task <id> --input <file>"),
        }
    }
    (
        task.unwrap_or_else(|| fail("GUIDANCE_USAGE", "provide --task <id>")),
        input.unwrap_or_else(|| fail("GUIDANCE_USAGE", "provide --input <file>")),
    )
}

pub fn run(args: &mut impl Iterator<Item = String>) {
    let command = args.next().unwrap_or_else(|| {
        fail(
            "GUIDANCE_USAGE",
            "select compile, status, a domain, plan, update, next, or close",
        )
    });
    let root = PathBuf::from(
        args.next()
            .unwrap_or_else(|| fail("GUIDANCE_USAGE", "provide the project root")),
    );
    match command.as_str() {
        "compile" => {
            if args.next().is_some() {
                fail("GUIDANCE_USAGE", "use appsdk guide compile <project>");
            }
            compile(&root);
        }
        "status" => {
            let (task, module) = parse_options(args);
            output(&project_status(
                &root,
                None,
                task.as_deref(),
                module.as_deref(),
                false,
            ));
        }
        "plan" => {
            let (task, input) = parse_mutation_options(args);
            plan(&root, &task, &input);
        }
        "update" => {
            let (task, input) = parse_mutation_options(args);
            update(&root, &task, &input);
        }
        "next" => {
            let (task, module) = parse_options(args);
            let task = task.unwrap_or_else(|| fail("GUIDANCE_USAGE", "provide --task <id>"));
            output(&project_status(
                &root,
                None,
                Some(&task),
                module.as_deref(),
                false,
            ));
        }
        "close" => {
            let (task, _) = parse_options(args);
            let task = task.unwrap_or_else(|| fail("GUIDANCE_USAGE", "provide --task <id>"));
            output(&close(&root, &task));
        }
        domain if DOMAINS.contains(&domain) => {
            let (task, module) = parse_options(args);
            output(&project_status(
                &root,
                Some(domain),
                task.as_deref(),
                module.as_deref(),
                true,
            ));
        }
        _ => fail(
            format!("GUIDANCE_DOMAIN_UNKNOWN:{}", command),
            "select compile, status, bootstrap, migration, governance-preflight, develop, debug, review, delivery, integration, promotion, freeze, cleanup, plan, update, next, or close",
        ),
    }
}
