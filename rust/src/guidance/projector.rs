use super::*;

fn missing_module_path<'a>(
    root: &Path,
    selected_module: &'a Value,
) -> Option<(&'static str, &'a str)> {
    for surface in ["owned_paths", "contract_paths"] {
        for value in required_array(selected_module, surface, "GUIDANCE_MODULE_SURFACE_INVALID") {
            let relative = value.as_str().unwrap_or_else(|| {
                fail(
                    "GUIDANCE_MODULE_SURFACE_INVALID",
                    "declare project-relative module paths",
                )
            });
            let base = safe_relative(normalize_surface(relative), "GUIDANCE_MODULE_PATH_ESCAPE");
            assert_no_symlink(root, &base, "GUIDANCE_MODULE_PATH_SYMLINK");
            if !root.join(base).exists() {
                return Some((surface, relative));
            }
        }
    }
    None
}

pub(super) fn step_events<'a>(events: &'a [Value], plan_hash: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| {
            event.get("record_type").and_then(Value::as_str) == Some("StepExecutionRecord")
                && event.get("plan_hash").and_then(Value::as_str) == Some(plan_hash)
        })
        .collect()
}

pub(super) fn next_step<'a>(
    plan: &'a Value,
    events: &[&Value],
) -> Result<Option<&'a Value>, &'static str> {
    let steps = required_array(plan, "steps", "GUIDANCE_PLAN_INVALID");
    for step in steps {
        let step_id = required_string(step, "step_id", "GUIDANCE_PLAN_INVALID");
        let event = events
            .iter()
            .find(|event| event.get("step_id").and_then(Value::as_str) == Some(step_id));
        match event.and_then(|event| event.get("result").and_then(Value::as_str)) {
            Some("pass") => continue,
            Some("fail" | "blocked") => return Err("STEP_BLOCKED"),
            Some(_) => return Err("EVENT_INVALID"),
            None => return Ok(Some(step)),
        }
    }
    Ok(None)
}

pub(super) fn project_status(
    root: &Path,
    domain: Option<&str>,
    task: Option<&str>,
    requested_module: Option<&str>,
    include_prompt: bool,
) -> Value {
    let project = read_project(root);
    let selected_module = module(&project, requested_module);
    let lifecycle = lifecycle_projection(&project, selected_module);
    let enforcement = guidance_contract(&project)
        .and_then(|guidance| guidance.get("enforcement"))
        .and_then(Value::as_str)
        .unwrap_or("advisory");
    if guidance_contract(&project).is_none() {
        let module_id = required_string(selected_module, "module_id", "GUIDANCE_MODULE_INVALID");
        return serde_json::json!({
            "harness": "appsdk-development-process-control",
            "enforcement": enforcement,
            "lifecycle": lifecycle,
            "readiness": "needs_setup",
            "reason_code": "GUIDANCE_SETUP_REQUIRED",
            "first_failing_gate": null,
            "guide_flow_required": enforcement == "forbidden",
            "next": {
                "command": format!("appsdk guide init --task guidance-setup --mode bootstrap --module {}", module_id),
                "then": "read project documents, present GuidanceSetupProposal, and wait for explicit user approval before durable rule writes or compile"
            }
        });
    }
    if let Some((surface, path)) = missing_module_path(root, selected_module) {
        let module_id = required_string(selected_module, "module_id", "GUIDANCE_MODULE_INVALID");
        let owner = required_string(
            selected_module,
            "source_owner",
            "GUIDANCE_MODULE_OWNER_INVALID",
        );
        return serde_json::json!({
            "harness": "appsdk-development-process-control",
            "enforcement": enforcement,
            "lifecycle": lifecycle,
            "readiness": "blocked",
            "reason_code": format!("MODULE_PATH_MISSING:{}:{}", module_id, path),
            "first_failing_gate": "module_binding",
            "retry_allowed": false,
            "preserved_state": true,
            "next": {
                "owner": owner,
                "surface": surface,
                "path": path,
                "action": "bind this module surface in .appsdk/project.json to an existing project-owned path, then compile guidance and verify the project"
            }
        });
    }
    let Some(compiled) = load_compiled(root, &project) else {
        let domain = domain.unwrap_or("governance-preflight");
        let module_id = required_string(selected_module, "module_id", "GUIDANCE_MODULE_INVALID");
        let task_id = task.unwrap_or("<task-id>");
        return serde_json::json!({
            "harness": "appsdk-development-process-control",
            "enforcement": enforcement,
            "lifecycle": lifecycle,
            "readiness": "ready",
            "reason_code": "GUIDANCE_NOT_COMPILED",
            "first_failing_gate": null,
            "guide_flow_required": enforcement == "forbidden",
            "next": {
                "command": "appsdk guide compile",
                "then": format!("appsdk guide init --task {} --mode {} --module {}", task_id, domain, module_id)
            }
        });
    };
    if let Some(reason) = compiled_context_drift(root, &project, &compiled) {
        return serde_json::json!({
            "harness": "appsdk-development-process-control",
            "enforcement": enforcement,
            "lifecycle": lifecycle,
            "readiness": "blocked",
            "reason_code": format!("GUIDANCE_COMPILED_CONTEXT_DRIFT:{}", reason),
            "first_failing_gate": "guidance_compile",
            "next": {"command": "appsdk guide compile"}
        });
    }
    if let Some(task) = task {
        let path = plan_file(root, task);
        if path.is_file() {
            let plan = read_plan(root, task);
            let events = read_events(root, task);
            let plan_hash = required_string(&plan, "plan_hash", "GUIDANCE_PLAN_INVALID");
            let current_events = step_events(&events, plan_hash);
            if let Some(reason) = context_drift(root, &project, &compiled, &plan) {
                let revision_reason = match reason {
                    "source" => "source_drift",
                    "goal" | "rule_context" => "rule_context_changed",
                    "project_contract" => "project_contract_changed",
                    "guidance_manifest" => "guidance_manifest_changed",
                    "owner" => "owner_changed",
                    "scope" => "scope_changed",
                    _ => "rule_context_changed",
                };
                return serde_json::json!({
                    "harness": "appsdk-development-process-control",
                    "enforcement": enforcement,
                    "domain": plan["mode"],
                    "task_id": task,
                    "lifecycle": lifecycle,
                    "readiness": "blocked",
                    "reason_code": format!("GUIDANCE_CONTEXT_DRIFT:{}", reason),
                    "first_failing_gate": "rule_context",
                    "next": {"command": "appsdk guide plan --task <id> --input <revised-plan>", "revision_reason": revision_reason}
                });
            }
            return match next_step(&plan, &current_events) {
                Ok(Some(step)) => serde_json::json!({
                    "harness": "appsdk-development-process-control",
                    "enforcement": enforcement,
                    "domain": plan["mode"],
                    "task_id": task,
                    "lifecycle": lifecycle,
                    "readiness": "ready",
                    "reason_code": "NEXT_STEP_READY",
                    "first_failing_gate": null,
                    "next": {"step_id": step["step_id"], "node_id": step["node_id"], "action": step["action"], "expected_evidence": step["expected_evidence"]}
                }),
                Ok(None) => serde_json::json!({
                    "harness": "appsdk-development-process-control",
                    "enforcement": enforcement,
                    "domain": plan["mode"],
                    "task_id": task,
                    "lifecycle": lifecycle,
                    "readiness": "complete",
                    "reason_code": "WORKFLOW_COMPLETE",
                    "first_failing_gate": null,
                    "next": null
                }),
                Err(reason) => serde_json::json!({
                    "harness": "appsdk-development-process-control",
                    "enforcement": enforcement,
                    "domain": plan["mode"],
                    "task_id": task,
                    "lifecycle": lifecycle,
                    "readiness": "blocked",
                    "reason_code": reason,
                    "first_failing_gate": "step_result",
                    "next": {"command": "appsdk guide plan --task <id> --input <revised-plan>", "revision_reason": "new_blocker"}
                }),
            };
        }
    }
    let domain = domain.unwrap_or("governance-preflight");
    let selected_workflow = workflow(&compiled, domain);
    let entry = required_string(selected_workflow, "entry_node", "GUIDANCE_WORKFLOW_INVALID");
    let node = required_array(selected_workflow, "nodes", "GUIDANCE_WORKFLOW_INVALID")
        .iter()
        .find(|node| node.get("node_id").and_then(Value::as_str) == Some(entry))
        .unwrap_or_else(|| {
            fail(
                "GUIDANCE_WORKFLOW_INVALID",
                "recompile the guidance manifest",
            )
        });
    let mut value = serde_json::json!({
        "harness": "appsdk-development-process-control",
        "enforcement": enforcement,
        "domain": domain,
        "lifecycle": lifecycle,
        "readiness": "ready",
        "reason_code": "PLAN_REQUIRED",
        "first_failing_gate": null,
        "rule_sources": compiled["sources"],
        "next": {"node_id": entry, "instruction": node["instruction"], "expected_evidence": node["evidence"]}
    });
    if include_prompt {
        value["plan_prompt"] = Value::String(
            "Read only declared rule sources. Generate schema_version 1 PlanProposal for this domain. Use the projected entry, canonical owner, module-owned scope, adjacent nodes, and required evidence. Do not output current_node, next_transition, or hashes."
                .into(),
        );
    }
    let module_id = required_string(selected_module, "module_id", "GUIDANCE_MODULE_INVALID");
    let task_id = task.unwrap_or("<task-id>");
    value["guide_flow_required"] = Value::Bool(enforcement == "forbidden");
    value["init_command"] = Value::String(format!(
        "appsdk guide init --task {} --mode {} --module {}",
        task_id, domain, module_id
    ));
    value
}

pub(super) fn close(root: &Path, task: &str) -> Value {
    let status = project_status(root, None, Some(task), None, false);
    let lifecycle_complete = status
        .pointer("/lifecycle/module_stage")
        .and_then(Value::as_str)
        .is_some_and(|stage| matches!(stage, "frozen" | "retired"));
    let workflow_complete = status.get("readiness").and_then(Value::as_str) == Some("complete");
    let domain = status.get("domain").and_then(Value::as_str);
    let freeze_in_scope = matches!(domain, Some("freeze" | "promotion"));
    serde_json::json!({
        "harness": "appsdk-development-process-control",
        "task_id": task,
        "workflow_complete": workflow_complete,
        "appsdk_lifecycle_complete": lifecycle_complete,
        "status": status,
        "remaining_gaps": if freeze_in_scope && !lifecycle_complete { serde_json::json!(["canonical AppSDK lifecycle is not frozen or retired"]) } else { serde_json::json!([]) },
        "cleanup_required": domain == Some("cleanup") && !workflow_complete,
        "memory_candidates": [],
        "memory_applied": false
    })
}
