use super::*;

fn command(mode: &str, task: &str, module_id: &str) -> String {
    format!(
        "appsdk guide {} <project> --task {} --module {}",
        mode, task, module_id
    )
}

fn questions(mode: &str, goal: &Value) -> Vec<Value> {
    let mut values = vec![
        serde_json::json!({
            "question_id": "objective_confirmation",
            "prompt": "After reading the declared AGENTS and Skills, confirm the bounded objective, non-goals, and acceptance evidence with the user.",
            "required": true
        }),
        serde_json::json!({
            "question_id": "scope_owner_confirmation",
            "prompt": "Confirm the module owner, allowed scope, forbidden paths, dependencies, and affected runtime entrypoint.",
            "required": true
        }),
    ];
    match mode {
        "develop" => values.extend([
            serde_json::json!({
                "question_id": "requirements_confirmation",
                "prompt": "Confirm the requirements, user-visible behavior, edge cases, and exact completion evidence before design.",
                "required": true
            }),
            serde_json::json!({
                "question_id": "architecture_confirmation",
                "prompt": "Confirm the top-down module map, outline design, detailed design, and logical closure before implementation.",
                "required": true
            }),
            serde_json::json!({
                "question_id": "delivery_depth",
                "prompt": "Confirm whether acceptance ends at tests/build or also requires install, restart, deployed entrypoint replay, review, merge, promotion, and freeze.",
                "required": true
            }),
        ]),
        "debug" => values.extend([
            serde_json::json!({
                "question_id": "failure_sample",
                "prompt": "Confirm the exact failing sample, expected behavior, actual behavior, and reproducible entrypoint.",
                "required": true
            }),
            serde_json::json!({
                "question_id": "causal_evidence",
                "prompt": "Confirm the active hypothesis, confirmation and falsification signals, first divergence, and forward/reversal experiment.",
                "required": true
            }),
            serde_json::json!({
                "question_id": "regression_replay",
                "prompt": "Confirm the exact old sample and deployed entrypoint that must be replayed after the root fix.",
                "required": true
            }),
        ]),
        _ => values.push(serde_json::json!({
            "question_id": "domain_completion",
            "prompt": "Confirm the current domain boundary, required evidence, and the lifecycle milestone that must follow this workflow.",
            "required": true
        })),
    }
    if let Some(open) = goal.get("questions").and_then(Value::as_array) {
        values.extend(open.iter().filter_map(|question| {
            if question.get("status").and_then(Value::as_str) != Some("open") {
                return None;
            }
            Some(serde_json::json!({
                "question_id": question.get("question_id").cloned().unwrap_or(Value::Null),
                "prompt": question.get("question").cloned().unwrap_or(Value::Null),
                "required": true,
                "source": ".appsdk/goal.json"
            }))
        }));
    }
    values
}

fn skill_commands(root: &Path, compiled: &Value) -> Vec<Value> {
    required_array(compiled, "sources", "GUIDANCE_COMPILED_INVALID")
        .iter()
        .filter(|source| {
            source.get("kind").and_then(Value::as_str) == Some("skill")
                && source.get("present").and_then(Value::as_bool) == Some(true)
        })
        .map(|source| {
            let skill_id = if let Some(contract) = source.get("contract_path").and_then(Value::as_str)
            {
                let relative = safe_relative(contract, "GUIDANCE_RULE_SOURCE_PATH_ESCAPE");
                assert_no_symlink(root, &relative, "GUIDANCE_RULE_SOURCE_SYMLINK");
                let manifest = read_json(&root.join(relative), "GUIDANCE_CONTRACT_INVALID");
                required_string(&manifest, "skill_id", "GUIDANCE_MANIFEST_INVALID").to_string()
            } else {
                let path_text = required_string(source, "path", "GUIDANCE_COMPILED_INVALID");
                let relative = safe_relative(path_text, "GUIDANCE_RULE_SOURCE_PATH_ESCAPE");
                relative
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    .filter(|name| valid_id(name))
                    .unwrap_or_else(|| {
                        fail(
                            "GUIDANCE_SKILL_INVOCATION_UNKNOWN",
                            "place the declared Skill at <skill-id>/SKILL.md or add its machine contract",
                        )
                    })
                    .to_string()
            };
            Some(serde_json::json!({
                "skill_id": skill_id,
                "command": format!("${}", skill_id),
                "source_path": source["path"]
            }))
        })
        .flatten()
        .collect()
}

pub(super) fn initialize(
    root: &Path,
    task: &str,
    mode: &str,
    requested_module: Option<&str>,
) -> Value {
    assert_id(task, "GUIDANCE_TASK_ID_INVALID");
    if !DOMAINS.contains(&mode) {
        fail(
            format!("GUIDANCE_DOMAIN_UNKNOWN:{}", mode),
            "select a declared guide domain for --mode",
        );
    }
    let project = read_project(root);
    let selected_module = module(&project, requested_module);
    let module_id = required_string(selected_module, "module_id", "GUIDANCE_MODULE_INVALID");
    let base = project_status(root, Some(mode), Some(task), Some(module_id), false);
    if base.get("reason_code").and_then(Value::as_str) == Some("GUIDANCE_NOT_COMPILED") {
        return serde_json::json!({
            "harness": "appsdk-development-process-control",
            "guide_flow_required": true,
            "task_id": task,
            "mode": mode,
            "module_id": module_id,
            "readiness": "needs_setup",
            "reason_code": "GUIDANCE_NOT_COMPILED",
            "writes_state": false,
            "missing_commands": [
                "appsdk guide compile <project>",
                format!("appsdk guide init <project> --task {} --mode {} --module {}", task, mode, module_id)
            ],
            "next": {"command": "appsdk guide compile <project>"}
        });
    }
    if base.get("readiness").and_then(Value::as_str) == Some("blocked") {
        let mut blocked = base;
        blocked["guide_flow_required"] = Value::Bool(true);
        blocked["writes_state"] = Value::Bool(false);
        return blocked;
    }
    if plan_file(root, task).is_file() {
        return serde_json::json!({
            "harness": "appsdk-development-process-control",
            "guide_flow_required": true,
            "task_id": task,
            "mode": mode,
            "module_id": module_id,
            "readiness": "ready",
            "reason_code": "GUIDANCE_PLAN_ALREADY_INITIALIZED",
            "writes_state": false,
            "next": {"command": format!("appsdk guide next <project> --task {}", task)}
        });
    }
    let compiled = load_compiled(root, &project).unwrap_or_else(|| {
        fail(
            "GUIDANCE_NOT_COMPILED",
            "run appsdk guide compile <project>",
        )
    });
    let goal = read_goal(root);
    let selected_workflow = workflow(&compiled, mode);
    let sources = required_array(&compiled, "sources", "GUIDANCE_COMPILED_INVALID");
    let read_first = sources
        .iter()
        .filter(|source| source.get("present").and_then(Value::as_bool) == Some(true))
        .map(|source| {
            serde_json::json!({
                "source_id": source["source_id"],
                "kind": source["kind"],
                "path": source["path"],
                "digest": source["digest"],
                "precedence": source["precedence"]
            })
        })
        .collect::<Vec<_>>();
    let missing_context = sources
        .iter()
        .filter(|source| source.get("present").and_then(Value::as_bool) != Some(true))
        .map(|source| {
            serde_json::json!({
                "source_id": source["source_id"],
                "kind": source["kind"],
                "path": source["path"],
                "required": source["required"],
                "action": "restore or declare the project-owned context when it is needed; optional absence is advisory"
            })
        })
        .collect::<Vec<_>>();
    let domain_command = command(mode, task, module_id);
    serde_json::json!({
        "harness": "appsdk-development-process-control",
        "guide_flow_required": true,
        "task_id": task,
        "mode": mode,
        "project_id": project["project_id"],
        "module": {
            "module_id": module_id,
            "owner": selected_module["source_owner"],
            "stage": selected_module["stage"],
            "owned_paths": selected_module["owned_paths"],
            "contract_paths": selected_module["contract_paths"]
        },
        "goal": {
            "goal_id": goal["goal_id"],
            "status": goal["status"],
            "objective": goal["understood_objective"],
            "acceptance_criteria": goal["acceptance_criteria"],
            "non_goals": goal["non_goals"],
            "ambiguities": goal["ambiguities"]
        },
        "workflow": {
            "objective": selected_workflow["objective"],
            "entry_node": selected_workflow["entry_node"]
        },
        "readiness": "ready",
        "reason_code": "GUIDANCE_INTAKE_REQUIRED",
        "writes_state": false,
        "read_first": read_first,
        "missing_context": missing_context,
        "skill_commands": skill_commands(root, &compiled),
        "questions": questions(mode, &goal),
        "agent_instruction": "Read the declared context in precedence order. Reconcile it with the current request. Ask the user only questions still unresolved, invoke relevant Skill commands, then enter the projected guide domain and submit the PlanProposal. Do not invent answers or hashes.",
        "command_sequence": [
            domain_command,
            format!("appsdk guide plan <project> --task {} --input <plan.json>", task),
            format!("appsdk guide next <project> --task {}", task)
        ],
        "next": {"command": command(mode, task, module_id)},
        "persistence": {"command": format!("appsdk guide plan <project> --task {} --input <plan.json>", task), "truth": ".appsdk-control/guidance/<task-id>/plan.json"}
    })
}
