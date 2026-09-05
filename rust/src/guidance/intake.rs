use super::*;

fn command(mode: &str, task: &str, module_id: &str) -> String {
    format!(
        "appsdk guide {} --task {} --module {}",
        mode, task, module_id
    )
}

fn readable_project_file(root: &Path, relative: &Path) -> Option<Vec<u8>> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        if fs::symlink_metadata(&current)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return None;
        }
    }
    fs::read(root.join(relative)).ok()
}

struct BootstrapSource<'a> {
    source_id: &'a str,
    kind: &'a str,
    path: &'a str,
    disposition: &'a str,
    contract_path: Option<&'a str>,
}

fn push_bootstrap_source(
    root: &Path,
    sources: &mut Vec<Value>,
    seen: &mut HashSet<String>,
    candidate: BootstrapSource<'_>,
) {
    if !seen.insert(candidate.path.to_string()) {
        return;
    }
    let relative = safe_relative(candidate.path, "GUIDANCE_BOOTSTRAP_SOURCE_PATH_ESCAPE");
    let Some(bytes) = readable_project_file(root, &relative) else {
        return;
    };
    let mut source = serde_json::json!({
        "source_id": candidate.source_id,
        "kind": candidate.kind,
        "path": candidate.path,
        "digest": digest_bytes(&bytes),
        "disposition": candidate.disposition
    });
    if candidate.disposition == "standard_reference" {
        source["required"] = Value::Bool(false);
        source["enforcement"] = Value::String("advisory".into());
    }
    if let Some(contract_path) = candidate.contract_path {
        let relative = safe_relative(contract_path, "GUIDANCE_BOOTSTRAP_CONTRACT_PATH_ESCAPE");
        if readable_project_file(root, &relative).is_some() {
            source["contract_path"] = Value::String(contract_path.to_string());
        }
    }
    sources.push(source);
}

fn local_skill_candidates(root: &Path) -> Vec<(String, String, Option<String>)> {
    let mut candidates = Vec::new();
    for base in ["skills", ".agents/skills", ".codex/skills"] {
        let base_relative = Path::new(base);
        let base_path = root.join(base_relative);
        if fs::symlink_metadata(&base_path)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
            || !base_path.is_dir()
        {
            continue;
        }
        let mut entries = fs::read_dir(&base_path)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let Some(skill_id) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if !valid_id(&skill_id) {
                continue;
            }
            let skill_path = format!("{}/{}/SKILL.md", base, skill_id);
            if readable_project_file(root, Path::new(&skill_path)).is_none() {
                continue;
            }
            let contract_path = format!("{}/{}/appsdk-guidance.json", base, skill_id);
            let contract_path = readable_project_file(root, Path::new(&contract_path))
                .is_some()
                .then_some(contract_path);
            candidates.push((skill_id, skill_path, contract_path));
        }
    }
    candidates
}

fn bootstrap_sources(root: &Path, project: &Value) -> Vec<Value> {
    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    if let Some(declared) = guidance_contract(project)
        .and_then(|guidance| guidance.get("rule_sources"))
        .and_then(Value::as_array)
    {
        for source in declared {
            let Some(path) = source.get("path").and_then(Value::as_str) else {
                continue;
            };
            push_bootstrap_source(
                root,
                &mut sources,
                &mut seen,
                BootstrapSource {
                    source_id: source
                        .get("source_id")
                        .and_then(Value::as_str)
                        .unwrap_or("declared-source"),
                    kind: source
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("skill"),
                    path,
                    disposition: "declared",
                    contract_path: source.get("contract_path").and_then(Value::as_str),
                },
            );
        }
    }
    push_bootstrap_source(
        root,
        &mut sources,
        &mut seen,
        BootstrapSource {
            source_id: "project-agents",
            kind: "agents",
            path: "AGENTS.md",
            disposition: "candidate",
            contract_path: None,
        },
    );
    push_bootstrap_source(
        root,
        &mut sources,
        &mut seen,
        BootstrapSource {
            source_id: "appsdk-governance-skill",
            kind: "skill",
            path: ".appsdk/skills/appsdk-project-governance/SKILL.md",
            disposition: "candidate",
            contract_path: Some(".appsdk/skills/appsdk-project-governance/appsdk-guidance.json"),
        },
    );
    for (index, (skill_id, path, contract_path)) in
        local_skill_candidates(root).into_iter().enumerate()
    {
        push_bootstrap_source(
            root,
            &mut sources,
            &mut seen,
            BootstrapSource {
                source_id: &format!("candidate-local-skill-{}-{}", index + 1, skill_id),
                kind: "skill",
                path: &path,
                disposition: "candidate",
                contract_path: contract_path.as_deref(),
            },
        );
    }
    push_bootstrap_source(
        root,
        &mut sources,
        &mut seen,
        BootstrapSource {
            source_id: "appsdk-standard-project-agent-template",
            kind: "template",
            path: ".appsdk/templates/minimal/AGENTS.md",
            disposition: "standard_reference",
            contract_path: None,
        },
    );
    sources
}

fn bootstrap_skill_commands(sources: &[Value]) -> Vec<Value> {
    let mut seen = HashSet::new();
    sources
        .iter()
        .filter(|source| source.get("kind").and_then(Value::as_str) == Some("skill"))
        .filter_map(|source| {
            let path = source.get("path").and_then(Value::as_str)?;
            let skill_id = Path::new(path)
                .parent()?
                .file_name()?
                .to_str()
                .filter(|value| valid_id(value))?;
            if !seen.insert(skill_id.to_string()) {
                return None;
            }
            Some(serde_json::json!({
                "skill_id": skill_id,
                "command": format!("${}", skill_id),
                "source_path": path,
                "status": "candidate_until_user_approval"
            }))
        })
        .collect()
}

fn bootstrap_setup(root: &Path, project: &Value, selected_module: &Value, task: &str) -> Value {
    let flow_required = project
        .pointer("/guidance/enforcement")
        .and_then(Value::as_str)
        == Some("forbidden");
    let module_id = required_string(selected_module, "module_id", "GUIDANCE_MODULE_INVALID");
    let sources = bootstrap_sources(root, project);
    let skill_commands = bootstrap_skill_commands(&sources);
    let guidance_compiled = root.join(compiled_relative(project)).is_file();
    let setup_kind = if guidance_compiled {
        "template_upgrade_review"
    } else {
        "initial_setup"
    };
    let reason_code = if guidance_compiled {
        "GUIDANCE_TEMPLATE_UPGRADE_PROPOSAL_REQUIRED"
    } else {
        "GUIDANCE_SETUP_PROPOSAL_REQUIRED"
    };
    let standard_template = sources
        .iter()
        .find(|source| {
            source.get("source_id").and_then(Value::as_str)
                == Some("appsdk-standard-project-agent-template")
        })
        .map(|source| {
            serde_json::json!({
                "path": source["path"],
                "version": env!("CARGO_PKG_VERSION"),
                "digest": source["digest"],
                "disposition": "standard_reference",
                "enforcement": "advisory"
            })
        })
        .unwrap_or(Value::Null);
    let current_sources = sources
        .iter()
        .filter(|source| {
            source.get("source_id").and_then(Value::as_str)
                != Some("appsdk-standard-project-agent-template")
        })
        .cloned()
        .collect::<Vec<_>>();
    serde_json::json!({
        "harness": "appsdk-development-process-control",
        "guide_flow_required": flow_required,
        "task_id": task,
        "mode": "bootstrap",
        "setup_kind": setup_kind,
        "project_id": project["project_id"],
        "module": {
            "module_id": module_id,
            "owner": selected_module["source_owner"],
            "stage": selected_module["stage"],
            "owned_paths": selected_module["owned_paths"],
            "contract_paths": selected_module["contract_paths"]
        },
        "readiness": "needs_user_approval",
        "reason_code": reason_code,
        "writes_state": false,
        "standard_template": standard_template,
        "existing_governance": {
            "project_contract": ".appsdk/project.json",
            "project_stage": project.pointer("/lifecycle/stage"),
            "guidance_declared": guidance_contract(project).is_some(),
            "guidance_compiled": root.join(compiled_relative(project)).is_file(),
            "records_root": ".appsdk/records",
            "maps_root": ".appsdk/maps",
            "active_root": "active",
            "protected_root": "protected",
            "preserved": true
        },
        "read_first": sources,
        "skill_commands": skill_commands,
        "questions": [
            {
                "question_id": "standard_template_comparison",
                "prompt": "Read current project rules first, then compare them with the installed AppSDK standard template. Recommend only useful differences; retain project decisions and record declined template items without forcing adoption.",
                "required": true
            },
            {
                "question_id": "standard_workflows",
                "prompt": "After reading the project documents, confirm the reusable develop, debug, review, delivery, integration, promotion, freeze, and cleanup flows; ask only unresolved questions.",
                "required": true
            },
            {
                "question_id": "project_commands",
                "prompt": "Confirm the project-owned test, build, install, restart, deployed-entrypoint replay, review, merge, and cleanup commands and the evidence each command produces.",
                "required": true
            },
            {
                "question_id": "rule_ownership",
                "prompt": "Confirm which facts remain in AGENTS.md, which reusable procedures belong in project-local Skills, and which nodes, edges, gates, severities, commands, and evidence contracts belong in machine guidance.",
                "required": true
            },
            {
                "question_id": "approval_boundary",
                "prompt": "Present the GuidanceSetupProposal and obtain explicit user approval before modifying durable project rule sources or compiling guidance.",
                "required": true
            }
        ],
        "proposal_schema": {
            "schema_version": 1,
            "proposal_type": "GuidanceSetupProposal",
            "setup_kind": setup_kind,
            "task_id": task,
            "project_id": project["project_id"],
            "module_id": module_id,
            "objective": "Describe the reusable project development process to standardize.",
            "standard_template": standard_template,
            "current_sources": current_sources,
            "recommended_changes": [],
            "retained_project_rules": [],
            "declined_template_items": [],
            "approval_required": true,
            "rule_sources": [],
            "workflows": [],
            "project_commands": [],
            "rule_classification": {
                "advisory": [],
                "warning": [],
                "forbidden": []
            },
            "unresolved_questions": [],
            "target_writes": {
                "project_facts": "AGENTS.md",
                "agent_procedures": ["<project-local-skill>/SKILL.md"],
                "machine_contracts": ["<project-local-skill>/appsdk-guidance.json"],
                "source_declaration": ".appsdk/project.json#/guidance"
            }
        },
        "agent_instruction": "Read current project AGENTS and Skills before the AppSDK standard template. Treat the template as an advisory versioned reference, compare differences, retain project decisions, and recommend only useful upgrades. Ask only unresolved questions, then present one GuidanceSetupProposal. Do not modify AGENTS.md, Skills, machine contracts, project.json, lifecycle records, Active, Protected, compiled guidance, or task state before explicit user approval.",
        "after_user_approval": {
            "actions": [
                "Use a clean owner worktree from latest origin/main.",
                "Apply only user-approved differences while preserving retained project rules; update project facts in AGENTS.md, reusable agent procedure in project-local Skills, and nodes, edges, gates, severities, commands, and evidence contracts in machine guidance as needed.",
                "Declare only the approved sources in .appsdk/project.json#/guidance/rule_sources."
            ],
            "commands": [
                "appsdk guide compile",
                "appsdk verify",
                format!("appsdk guide init --task <task-id> --mode <develop|debug> --module {}", module_id)
            ]
        },
        "next": {
            "action": "agent_reads_context_and_presents_guidance_setup_proposal",
            "requires": "explicit_user_approval",
            "writes_state": false
        }
    })
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
            serde_json::json!({
                "skill_id": skill_id,
                "command": format!("${}", skill_id),
                "source_path": source["path"]
            })
        })
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
    if mode == "bootstrap" {
        return bootstrap_setup(root, &project, selected_module, task);
    }
    let base = project_status(root, Some(mode), Some(task), Some(module_id), false);
    let flow_required = project
        .pointer("/guidance/enforcement")
        .and_then(Value::as_str)
        == Some("forbidden");
    if base.get("reason_code").and_then(Value::as_str) == Some("GUIDANCE_NOT_COMPILED") {
        return serde_json::json!({
            "harness": "appsdk-development-process-control",
            "guide_flow_required": flow_required,
            "task_id": task,
            "mode": mode,
            "module_id": module_id,
            "readiness": "needs_setup",
            "reason_code": "GUIDANCE_NOT_COMPILED",
            "writes_state": false,
            "missing_commands": [
                "appsdk guide compile",
                format!("appsdk guide init --task {} --mode {} --module {}", task, mode, module_id)
            ],
            "next": {"command": "appsdk guide compile"}
        });
    }
    if base.get("readiness").and_then(Value::as_str) == Some("blocked") {
        let mut blocked = base;
        blocked["guide_flow_required"] = Value::Bool(flow_required);
        blocked["writes_state"] = Value::Bool(false);
        return blocked;
    }
    if plan_file(root, task).is_file() {
        return serde_json::json!({
            "harness": "appsdk-development-process-control",
            "guide_flow_required": flow_required,
            "task_id": task,
            "mode": mode,
            "module_id": module_id,
            "readiness": "ready",
            "reason_code": "GUIDANCE_PLAN_ALREADY_INITIALIZED",
            "writes_state": false,
            "next": {"command": format!("appsdk guide next --task {}", task)}
        });
    }
    let compiled = load_compiled(root, &project)
        .unwrap_or_else(|| fail("GUIDANCE_NOT_COMPILED", "run appsdk guide compile"));
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
        "guide_flow_required": flow_required,
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
            format!("appsdk guide plan --task {} --input <plan.json>", task),
            format!("appsdk guide next --task {}", task)
        ],
        "next": {"command": command(mode, task, module_id)},
        "persistence": {"command": format!("appsdk guide plan --task {} --input <plan.json>", task), "truth": ".appsdk-control/guidance/<task-id>/plan.json"}
    })
}
