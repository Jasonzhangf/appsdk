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
    precedence: Option<u64>,
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
    if readable_project_file(root, &relative).is_none() {
        return;
    }
    let mut source = serde_json::json!({
        "source_id": candidate.source_id,
        "kind": candidate.kind,
        "path": candidate.path,
        "disposition": candidate.disposition
    });
    if let Some(precedence) = candidate.precedence {
        source["precedence"] = Value::Number(precedence.into());
    }
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
                    precedence: source.get("precedence").and_then(Value::as_u64),
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
            precedence: None,
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
            precedence: None,
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
                precedence: None,
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
            precedence: None,
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
                "version": env!("APPSDK_VERSION"),
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
    let proposal_schema = serde_json::json!({
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
        "recommended_change_required_fields": [
            "path",
            "owner",
            "action",
            "basis",
            "retained_safeguards",
            "affected_entrypoints"
        ],
        "retained_project_rules": [],
        "declined_template_items": [],
        "approval_required": "uncovered_durable_changes_only",
        "authorization_reuse": "existing_session_authorization",
        "approval_evidence": "conversation_authorization_or_explicit_user_approval",
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
    });
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
        "readiness": "needs_conditional_authorization",
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
                "prompt": "Read the effective upper-level rules, local AGENTS and Skills, actual test commands, and CI/hook behavior first. Compare the actual entrypoints with the installed AppSDK standard template. Audit each difference for deletion, merge, scope reduction, or necessary addition; retain project decisions and record declined template items. Reuse valid audit evidence when no relevant resource changed.",
                "required": true
            },
            {
                "question_id": "standard_workflows",
                "prompt": "Resolve the applicable develop, debug, review, delivery, integration and cleanup flows from the project documents and existing authorization. Ask only material unresolved questions. Promotion and freeze apply only to an explicitly selected release flow, never ordinary development.",
                "required": true
            },
            {
                "question_id": "project_commands",
                "prompt": "Confirm the project-owned test, build, install, restart, deployed-entrypoint replay, review, merge, and cleanup commands, their evidence, the risk-based selection for ordinary changes, and the full release gate.",
                "required": true
            },
            {
                "question_id": "rule_ownership",
                "prompt": "Confirm which facts remain in AGENTS.md, which reusable procedures belong in project-local Skills, and which nodes, edges, gates, severities, commands, and evidence contracts belong in machine guidance. Keep one owner per rule and remove confirmed duplication or unnecessary structure.",
                "required": true
            },
            {
                "question_id": "approval_boundary",
                "prompt": "Compare recommended durable changes with the existing conversation authorization. Reuse authorization that already covers a difference, and ask the user only about uncovered durable changes. Guidance compilation is optional and is not required when Guidance is not selected.",
                "required": true
            }
        ],
        "proposal_schema": proposal_schema,
        "agent_instruction": "Read the effective upper-level rules, local AGENTS and Skills, actual test commands, and CI/hook behavior before the advisory AppSDK template. Correct only affected contradictions and confirmed duplication at the unique owner; record material decisions in existing task notes without a separate difference ledger. Reuse existing authorization and valid audit evidence. Ask only about uncovered durable changes. Do not require Guidance compilation when Guidance is not selected. Do not modify out-of-scope project rules, contracts or runtime state.",
        "after_user_approval": {
            "condition": "only for uncovered durable changes; reuse existing session authorization when it covers the change",
            "actions": [
                "Use a clean owner worktree from latest origin/main.",
                "Apply only authorized differences while preserving retained project rules; update project facts in AGENTS.md, reusable agent procedures in project-local Skills, and machine guidance only when those owners are in scope.",
                "When Guidance is selected, declare only the authorized sources in .appsdk/project.json#/guidance/rule_sources.",
                "When Guidance is not selected, apply the authorized rule, CI, or hook changes without declaring Guidance or compiling it."
            ],
            "commands": [
                "appsdk guide compile",
                "appsdk verify",
                format!("appsdk guide init --task <task-id> --mode <develop|debug> --module {}", module_id)
            ],
            "compile_condition": "only when Guidance is selected and its rule sources are authorized"
        },
        "next": {
            "action": "agent_reads_effective_rules_and_presents_guidance_setup_proposal",
            "requires": "authorization_check_for_uncovered_durable_changes",
            "writes_state": false
        }
    })
}

fn open_questions(goal: &Value) -> Vec<Value> {
    let mut values = Vec::new();
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
        "questions": open_questions(&goal),
        "agent_instruction": "Read the declared context in precedence order. Resolve internal technical, scope, ownership, and collaboration questions through the owning Skill or Collab peer. Ask the user only for user-exclusive facts, new authorization, or hard-constraint changes. Invoke relevant Skill commands, then enter the projected guide domain and submit the PlanProposal. Do not invent answers or hashes.",
        "next": {"command": command(mode, task, module_id)},
    })
}
