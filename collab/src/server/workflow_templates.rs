// Standard workflow templates, project-scoped overrides, and their context
// projection. The compiled asset is the only source of the fixed baseline and
// coordination text; there is no specialty fallback.

const WORKFLOW_TEMPLATE_ASSET: &str =
    include_str!("../../skills/collab/references/workflow-templates.json");
const WORKFLOW_TEMPLATE_MASTER_REQUIRED: &str =
    "WORKFLOW_TEMPLATE_MASTER_REQUIRED: only the current master may set workflow templates";
const WORKFLOW_TEMPLATE_TARGET_INVALID: &str = "WORKFLOW_TEMPLATE_TARGET_INVALID: target must be master, peer, or role:<specialty> without control characters or commas";

#[derive(Debug, serde::Deserialize)]
struct WorkflowTemplateAsset {
    base_version: u32,
    identity_templates: std::collections::BTreeMap<
        String,
        crate::board::WorkflowTemplateSpec,
    >,
    coordination_rules: std::collections::BTreeMap<String, String>,
}

fn workflow_template_asset() -> &'static WorkflowTemplateAsset {
    static ASSET: std::sync::OnceLock<WorkflowTemplateAsset> = std::sync::OnceLock::new();
    ASSET.get_or_init(|| {
        serde_json::from_str(WORKFLOW_TEMPLATE_ASSET)
            .expect("embedded workflow-templates.json must be valid")
    })
}

/// Fixed common coordination text shared with the identity templates. These
/// rules stay in force even when a custom template omits them.
fn workflow_template_rule(key: &str) -> &'static str {
    workflow_template_asset()
        .coordination_rules
        .get(key)
        .map(String::as_str)
        .unwrap_or_else(|| panic!("unknown workflow template rule: {key}"))
}

fn workflow_base_template(base_role: &str) -> &'static crate::board::WorkflowTemplateSpec {
    let asset = workflow_template_asset();
    asset
        .identity_templates
        .get(base_role)
        .expect("workflow asset must define master and peer identity templates")
}

fn workflow_base_card(
    target: &str,
    spec: &crate::board::WorkflowTemplateSpec,
) -> serde_json::Value {
    json!({
        "target": target,
        "source": "base",
        "base_version": workflow_template_asset().base_version,
        "revision": 0,
        "template": spec,
    })
}

fn workflow_override_card(
    target: &str,
    entry: &crate::board::WorkflowTemplateOverride,
) -> serde_json::Value {
    json!({
        "target": target,
        "source": "master_override",
        "base_version": entry.base_version,
        "revision": entry.revision,
        "template": entry.spec,
    })
}

/// Project the effective identity template plus the configured specialty cards
/// for this caller. Only a master projection exposes the stored override
/// catalog; peers see only their effective cards.
fn workflow_template_projection(
    state: &State,
    project_scope: &str,
    base_role: &str,
    roles: &[String],
) -> serde_json::Value {
    let identity_role = if base_role == "master" { "master" } else { "peer" };
    let configured = state.workflow_templates.get(project_scope);
    let identity = match configured.and_then(|targets| targets.get(identity_role)) {
        Some(entry) => workflow_override_card(identity_role, entry),
        None => workflow_base_card(identity_role, workflow_base_template(identity_role)),
    };

    let mut roles = roles.iter().map(String::as_str).collect::<Vec<_>>();
    roles.sort_unstable();
    roles.dedup();
    let mut specialties = Vec::new();
    for role in roles {
        let target = format!("role:{role}");
        if let Some(entry) = configured.and_then(|targets| targets.get(&target)) {
            specialties.push(workflow_override_card(&target, entry));
        }
    }

    let mut projection = json!({
        "base_version": workflow_template_asset().base_version,
        "identity": identity,
        "specialties": specialties,
    });
    if identity_role == "master" {
        let mut configured_templates = serde_json::Map::new();
        if let Some(targets) = configured {
            let mut entries = targets.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            for (target, entry) in entries {
                configured_templates.insert(
                    target.clone(),
                    json!({
                        "base_version": entry.base_version,
                        "revision": entry.revision,
                        "template": entry.spec,
                        "updated_by": entry.updated_by,
                        "updated_ms": entry.updated_ms,
                    }),
                );
            }
        }
        projection["configured_templates"] = serde_json::Value::Object(configured_templates);
    }
    projection
}

fn normalize_workflow_target(target: &str) -> Result<String, String> {
    let lowered = target.trim().to_lowercase();
    if lowered == "master" || lowered == "peer" {
        return Ok(lowered);
    }
    let Some(role) = lowered.strip_prefix("role:") else {
        return Err(WORKFLOW_TEMPLATE_TARGET_INVALID.into());
    };
    let role = role.trim();
    if role.is_empty() || role.chars().any(char::is_control) || role.contains(',') {
        return Err(WORKFLOW_TEMPLATE_TARGET_INVALID.into());
    }
    Ok(format!("role:{role}"))
}

#[derive(Debug)]
struct WorkflowTemplatePlan {
    target: String,
    outcome: &'static str,
    revision: u64,
    event: Option<Event>,
}

fn plan_workflow_template_set(
    state: &State,
    project_scope: &str,
    actor: &str,
    is_master: bool,
    target: &str,
    template: Option<crate::board::WorkflowTemplateSpec>,
    updated_ms: i64,
) -> Result<WorkflowTemplatePlan, String> {
    if !is_master {
        return Err(WORKFLOW_TEMPLATE_MASTER_REQUIRED.into());
    }
    let target = normalize_workflow_target(target)?;
    let existing = state
        .workflow_templates
        .get(project_scope)
        .and_then(|targets| targets.get(&target));
    let Some(template) = template else {
        let event = existing.map(|_| Event::WorkflowTemplateUpdated {
            project_scope: project_scope.into(),
            target: target.clone(),
            entry: None,
        });
        return Ok(WorkflowTemplatePlan {
            target,
            outcome: "reset",
            revision: 0,
            event,
        });
    };
    let spec = template
        .normalized()
        .map_err(|error| format!("WORKFLOW_TEMPLATE_INVALID: {error}"))?;
    if let Some(existing) = existing {
        if existing.spec == spec {
            return Ok(WorkflowTemplatePlan {
                target,
                outcome: "unchanged",
                revision: existing.revision,
                event: None,
            });
        }
    }
    let revision = existing.map_or(1, |entry| entry.revision + 1);
    let entry = crate::board::WorkflowTemplateOverride {
        base_version: workflow_template_asset().base_version,
        revision,
        spec,
        updated_by: actor.into(),
        updated_ms,
    };
    Ok(WorkflowTemplatePlan {
        target: target.clone(),
        outcome: "updated",
        revision,
        event: Some(Event::WorkflowTemplateUpdated {
            project_scope: project_scope.into(),
            target,
            entry: Some(entry),
        }),
    })
}

fn handle_workflow_template_set(
    server: &Server,
    actor: String,
    token: String,
    target: String,
    template: Option<crate::board::WorkflowTemplateSpec>,
) -> Resp {
    let mut state = server.state.lock().unwrap();
    if let Err(error) = verify(&state, &actor, &token) {
        return error;
    }
    let route = match server_route_scope(server, &state) {
        Ok(Some(route)) => route,
        _ => return Resp::err("WORKFLOW_TEMPLATE_PROJECT_REQUIRED: project route is not ready"),
    };
    let is_master = match current_master_holder(server, &state) {
        Ok(Some(master)) => master == actor,
        Ok(None) => false,
        Err(error) => return Resp::err(error),
    };
    let plan = match plan_workflow_template_set(
        &state,
        route.project_scope_id.as_str(),
        &actor,
        is_master,
        &target,
        template,
        now_ms(),
    ) {
        Ok(plan) => plan,
        Err(error) => return Resp::err(error),
    };
    let mut notices = Vec::new();
    if let Some(event) = plan.event {
        let mut recipients: Vec<_> = state.workers.keys().filter(|id| **id != actor && !is_managed_subagent(&state, id))
            .filter(|id| plan.target == "peer" || plan.target.strip_prefix("role:").is_some_and(|role|
                worker_work_roles(&state, route.project_scope_id.as_str(), id).iter().any(|value| value == role)))
            .cloned().collect();
        recipients.sort();
        for recipient in recipients {
            notices.extend(relationship_notice_events(&state, route.project_scope_id.as_str(), &recipient,
                "workflow-template-changed", format!("流程模板 {} 已{}。读取 collab context 获取当前模板。已接任务的归属、指定执行者、reviewer 和验收条件保持；只协调受影响的职责或交接。", plan.target, if plan.outcome == "reset" {"重置"} else {"更新"})));
        }
        let mut events = vec![event];
        events.extend(notices.clone());
        if let Err(error) = server.commit_locked_checked(&mut state, &events) {
            return Resp::err(format!("WORKFLOW_TEMPLATE_DURABILITY_FAILED: {error}"));
        }
    }
    drop(state);
    let notifications = deliver_relationship_notices(server, &notices);
    Resp::data(json!({
        "target": plan.target,
        "outcome": plan.outcome,
        "revision": plan.revision,
        "base_version": workflow_template_asset().base_version,
        "notifications": notifications,
    }))
}

#[cfg(test)]
mod workflow_template_state_tests {
    use super::*;
    use crate::server::name_state::NameOccupancy;
    use crate::server::state::{Event, State};
    use serde_json::json;

    fn spec(role_task: &str) -> crate::board::WorkflowTemplateSpec {
        crate::board::WorkflowTemplateSpec {
            role_task: role_task.into(),
            responsibilities: vec![format!("{role_task} responsibilities")],
            next_action: format!("{role_task} next"),
            completion_action: format!("{role_task} completion"),
        }
    }

    fn set_event(scope: &str, target: &str, revision: u64, role_task: &str) -> Event {
        Event::WorkflowTemplateUpdated {
            project_scope: scope.into(),
            target: target.into(),
            entry: Some(crate::board::WorkflowTemplateOverride {
                base_version: workflow_template_asset().base_version,
                revision,
                spec: spec(role_task),
                updated_by: "master".into(),
                updated_ms: revision as i64,
            }),
        }
    }

    #[test]
    fn projection_defaults_then_overrides_and_keeps_configured_roles_only() {
        let mut state = State::default();

        let base = workflow_template_projection(&state, "/project", "peer", &[]);
        assert_eq!(base["base_version"], json!(1));
        assert_eq!(base["identity"]["target"], json!("peer"));
        assert_eq!(base["identity"]["source"], json!("base"));
        assert_eq!(base["identity"]["base_version"], json!(1));
        assert_eq!(base["identity"]["revision"], json!(0));
        assert_eq!(base["specialties"], json!([]));
        assert!(base.get("configured_templates").is_none());

        state
            .apply_checked(&set_event("/project", "peer", 1, "Peer override"))
            .unwrap();
        state
            .apply_checked(&set_event("/project", "role:reviewer", 1, "Reviewer task"))
            .unwrap();
        state
            .apply_checked(&set_event("/project", "role:designer", 1, "Designer task"))
            .unwrap();

        let projection = workflow_template_projection(
            &state,
            "/project",
            "peer",
            &[
                "reviewer".into(),
                "designer".into(),
                "analyst".into(),
            ],
        );
        assert_eq!(projection["identity"]["source"], json!("master_override"));
        assert_eq!(projection["identity"]["revision"], json!(1));
        assert_eq!(
            projection["identity"]["template"]["role_task"],
            json!("Peer override")
        );
        let specialties = projection["specialties"]
            .as_array()
            .expect("specialty cards");
        assert_eq!(specialties.len(), 2);
        assert_eq!(specialties[0]["target"], json!("role:designer"));
        assert_eq!(
            specialties[0]["template"]["role_task"],
            json!("Designer task")
        );
        assert_eq!(specialties[1]["target"], json!("role:reviewer"));
        assert_eq!(
            specialties[1]["template"]["role_task"],
            json!("Reviewer task")
        );
        assert_eq!(specialties[0]["source"], json!("master_override"));

        let master = workflow_template_projection(&state, "/project", "master", &[]);
        assert_eq!(master["identity"]["target"], json!("master"));
        assert_eq!(master["identity"]["source"], json!("base"));
        let catalog = master["configured_templates"]
            .as_object()
            .expect("master catalog");
        assert_eq!(catalog.len(), 3);
        assert_eq!(catalog["role:designer"]["revision"], json!(1));
    }

    #[test]
    fn configured_templates_survive_snapshot_compaction_and_reset_clears_targets() {
        let mut state = State::default();
        state
            .apply_checked(&set_event("/project", "peer", 1, "Peer override"))
            .unwrap();
        state
            .apply_checked(&set_event("/project", "role:designer", 1, "Designer task"))
            .unwrap();

        let mut replay = State::default();
        for event in state.snapshot_events() {
            replay.apply_checked(&event).unwrap();
        }
        assert_eq!(state.workflow_templates, replay.workflow_templates);

        replay
            .apply_checked(&Event::WorkflowTemplateUpdated {
                project_scope: "/project".into(),
                target: "role:designer".into(),
                entry: None,
            })
            .unwrap();
        assert!(!replay.workflow_templates["/project"].contains_key("role:designer"));
        assert!(replay.workflow_templates["/project"].contains_key("peer"));

        replay
            .apply_checked(&Event::WorkflowTemplateUpdated {
                project_scope: "/project".into(),
                target: "peer".into(),
                entry: None,
            })
            .unwrap();
        assert!(!replay.workflow_templates.contains_key("/project"));
    }

    #[test]
    fn project_reset_removes_only_the_exact_scope() {
        let mut state = State::default();
        for scope in ["/project", "/other-project"] {
            state
                .apply_checked(&set_event(scope, "peer", 1, "Peer override"))
                .unwrap();
        }
        state
            .apply_checked(&Event::ProjectResetStarted {
                project_scope: "/project".into(),
                clear_local: true,
                worker_ids: vec![],
            })
            .unwrap();
        assert!(!state.workflow_templates.contains_key("/project"));
        assert!(state.workflow_templates.contains_key("/other-project"));
    }

    #[test]
    fn stable_name_session_handoff_keeps_role_template_selection() {
        let mut state = State::default();
        for (worker, session, revision) in [("old", "s1", 1), ("current", "s2", 2)] {
            state
                .apply_checked(&Event::NameResolutionCommitted {
                    project_scope: "/project".into(),
                    occupancy: NameOccupancy {
                        name: "Alice".into(),
                        worker_id: worker.into(),
                        session_id: session.into(),
                        occupancy_revision: revision,
                        updated_ms: revision as i64,
                    },
                    assignments: vec![],
                })
                .unwrap();
            if revision == 1 {
                state
                    .apply_checked(&Event::NameWorkRolesUpdated {
                        project_scope: "/project".into(),
                        record: crate::board::NameWorkRoles {
                            name: "Alice".into(),
                            work_roles: vec!["designer".into()],
                        },
                    })
                    .unwrap();
                state
                    .apply_checked(&set_event(
                        "/project",
                        "role:designer",
                        1,
                        "Designer task",
                    ))
                    .unwrap();
            }
        }
        assert!(worker_work_roles(&state, "/project", "old").is_empty());
        let roles = worker_work_roles(&state, "/project", "current");
        assert_eq!(roles, ["designer"]);
        let projection = workflow_template_projection(&state, "/project", "peer", &roles);
        assert_eq!(projection["specialties"][0]["target"], json!("role:designer"));
        assert_eq!(
            projection["specialties"][0]["template"]["role_task"],
            json!("Designer task")
        );
    }

    #[test]
    fn plan_is_idempotent_and_rejects_non_master_and_invalid_inputs() {
        let mut state = State::default();

        let rejected = plan_workflow_template_set(
            &state,
            "/project",
            "peer",
            false,
            "role:designer",
            Some(spec("Designer task")),
            1,
        )
        .unwrap_err();
        assert!(rejected.contains("WORKFLOW_TEMPLATE_MASTER_REQUIRED"));
        assert!(state.workflow_templates.is_empty());

        let invalid_target = plan_workflow_template_set(
            &state,
            "/project",
            "master",
            true,
            "designer",
            Some(spec("Designer task")),
            1,
        )
        .unwrap_err();
        assert!(invalid_target.contains("WORKFLOW_TEMPLATE_TARGET_INVALID"));

        let invalid_template = plan_workflow_template_set(
            &state,
            "/project",
            "master",
            true,
            "role:designer",
            Some(crate::board::WorkflowTemplateSpec {
                role_task: " ".into(),
                responsibilities: vec![],
                next_action: "next".into(),
                completion_action: "done".into(),
            }),
            1,
        )
        .unwrap_err();
        assert!(invalid_template.contains("WORKFLOW_TEMPLATE_INVALID"));
        assert!(state.workflow_templates.is_empty());

        let first = plan_workflow_template_set(
            &state,
            "/project",
            "master",
            true,
            "role:designer",
            Some(spec("Designer task")),
            10,
        )
        .unwrap();
        assert_eq!(first.outcome, "updated");
        assert_eq!(first.revision, 1);
        state
            .apply_checked(first.event.as_ref().expect("first set event"))
            .unwrap();

        let unchanged = plan_workflow_template_set(
            &state,
            "/project",
            "master",
            true,
            "role:designer",
            Some(spec("Designer task")),
            20,
        )
        .unwrap();
        assert_eq!(unchanged.outcome, "unchanged");
        assert_eq!(unchanged.revision, 1);
        assert!(unchanged.event.is_none());

        let changed = plan_workflow_template_set(
            &state,
            "/project",
            "master",
            true,
            "role:designer",
            Some(spec("Updated designer task")),
            30,
        )
        .unwrap();
        assert_eq!(changed.outcome, "updated");
        assert_eq!(changed.revision, 2);

        let reset = plan_workflow_template_set(
            &state,
            "/project",
            "master",
            true,
            "role:designer",
            None,
            40,
        )
        .unwrap();
        assert_eq!(reset.outcome, "reset");
        assert_eq!(reset.revision, 0);
        assert!(reset.event.is_some());
    }
}
