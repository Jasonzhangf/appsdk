use super::*;

fn workflow_spec(role_task: &str, responsibility: &str, next: &str, done: &str) -> String {
    json!({
        "role_task": role_task,
        "responsibilities": [responsibility],
        "next_action": next,
        "completion_action": done,
    })
    .to_string()
}

fn master_fixture() -> AppFixture {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"workflow template consumer"}"#,
        ],
        THREAD_A,
    );
    fixture
}

fn task<'a>(board: &'a Value, id: &str) -> &'a Value {
    board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == id)
        .unwrap()
}

fn assert_no_static_workflow_operations(context: &Value) {
    let operations = context["operations"].as_array().expect("operations");
    let copied = operations
        .iter()
        .filter_map(|operation| operation["kind"].as_str())
        .filter(|kind| {
            matches!(
                *kind,
                "responsibility" | "next_action" | "completion_action" | "notification_rule"
            )
        })
        .collect::<Vec<_>>();
    assert!(
        copied.is_empty(),
        "operations must contain current facts, not copied workflow text: {copied:?}"
    );
}

/// Both standard identity baselines expose the default workflow template at
/// base_version 1 before any master-configured override exists.
#[test]
fn context_workflow_template_default_base_version_for_master_and_peer() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    let master = fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"workflow template default consumer"}"#,
        ],
        THREAD_A,
    );
    let peer = fixture.run_public(&["context"], THREAD_B);

    for (label, snapshot, target) in [("master", &master, "master"), ("peer", &peer, "peer")] {
        let workflow = &snapshot["role_brief"]["workflow_template"];
        let brief = &snapshot["role_brief"];
        let responsibilities = brief["responsibilities"].as_array().unwrap();
        for route in [
            "Make ordinary technical decisions autonomously",
            "factual questions to the owning peer",
            "design questions to the designer",
            "scope, resources and conflicts to the current master",
            "user-exclusive facts, genuinely new authorization or explicit hard-constraint changes",
            "Reuse existing user authorization without asking again",
        ] {
            assert!(
                responsibilities
                    .iter()
                    .any(|item| item.as_str().is_some_and(|text| text.contains(route))),
                "{label}: missing clarification route {route}: {brief}"
            );
        }
        assert!(!brief["next_action"]
            .as_str()
            .unwrap()
            .contains("ask the current master first with evidence when blocked"));
        assert_eq!(
            workflow["base_version"],
            json!(1),
            "{label}: standard identity must expose workflow_template.base_version 1: {snapshot}"
        );
        assert_eq!(
            workflow["specialties"],
            json!([]),
            "{label}: no specialty template exists yet: {workflow}"
        );
        let identity = &workflow["identity"];
        assert_eq!(identity["target"], json!(target), "{label}: {identity}");
        assert_eq!(identity["source"], json!("base"), "{label}: {identity}");
        assert_eq!(identity["base_version"], json!(1), "{label}: {identity}");
        assert_eq!(identity["revision"], json!(0), "{label}: {identity}");
        assert!(
            identity.get("template").is_none(),
            "{label}: the workflow card must reference its source without embedding its text: {identity}"
        );
        for field in [
            "role_task",
            "responsibilities",
            "next_action",
            "completion_action",
        ] {
            assert!(
                !snapshot["role_brief"][field].is_null(),
                "{label}: role_brief must expose effective {field} once: {snapshot}"
            );
            if field == "responsibilities" {
                assert!(
                    !brief["responsibilities"]
                        .as_array()
                        .expect("responsibilities array")
                        .is_empty(),
                    "{label}: role_brief responsibilities must be nonempty: {snapshot}"
                );
            } else {
                assert!(
                    !brief[field]
                        .as_str()
                        .expect("role_brief text")
                        .trim()
                        .is_empty(),
                    "{label}: role_brief {field} must be nonempty: {snapshot}"
                );
            }
        }
    }
}

#[test]
fn context_identity_override_has_no_fixed_action_suffix() {
    let fixture = master_fixture();
    fixture.run_public(&["context"], THREAD_B);
    for (target, thread) in [("master", THREAD_A), ("peer", THREAD_B)] {
        let template = workflow_spec(
            "Resolve the owned project question",
            "Use the accepted scope and current task pins",
            "Ask the owning peer for the missing measurement, then continue",
            "Return the measurement to the assigned receiver",
        );
        let updated = fixture.run_public(
            &[
                "context",
                "--workflow-target",
                target,
                "--workflow-template",
                &template,
            ],
            THREAD_A,
        );
        let snapshot = if target == "master" {
            updated
        } else {
            fixture.run_public(&["context"], thread)
        };
        let brief = &snapshot["role_brief"];
        let selected = &brief["workflow_template"]["identity"];
        assert_eq!(selected["source"], "master_override");
        assert_eq!(selected["revision"], 1);
        assert!(
            selected.get("template").is_none(),
            "the selected card must reference its source without embedding its text: {selected}"
        );
        let expected: Value = serde_json::from_str(&template).expect("workflow spec");
        for field in ["role_task", "next_action", "completion_action"] {
            assert_eq!(
                brief[field], expected[field],
                "{target}: effective {field} must preserve the selected template"
            );
        }
        assert_no_static_workflow_operations(&snapshot);
    }
}

/// A master-configured role template is persisted through the mutation request
/// and is visible in the next `collab context` projection for a matching peer.
#[test]
fn context_workflow_template_flags_update_reaches_next_context() {
    let fixture = AppFixture::new();
    fixture.run_public(&["context"], THREAD_A);
    fixture.run_public(
        &[
            "context",
            "--approve-grant",
            r#"{"action":"restore_master","authorization":"workflow template update consumer"}"#,
        ],
        THREAD_A,
    );
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context", "--work-roles", "designer"], THREAD_B);
    let template = r#"{"role_task":"Draft the design proposal","responsibilities":["Deliver the design draft"],"next_action":"Publish the design draft","completion_action":"Report the design result"}"#;
    let updated = fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "role:designer",
            "--workflow-template",
            template,
        ],
        THREAD_A,
    );
    let receipt = &updated["workflow_template_update"];
    assert_eq!(receipt["target"], json!("role:designer"), "{updated}");
    assert_eq!(receipt["outcome"], json!("updated"), "{updated}");
    assert_eq!(receipt["revision"], json!(1), "{updated}");
    // The same invocation refresh must already project the mutation.
    assert_eq!(
        updated["role_brief"]["workflow_template"]["identity"]["target"],
        json!("master"),
        "{updated}"
    );

    let next = fixture.run_public(&["context"], THREAD_B);
    let workflow = &next["role_brief"]["workflow_template"];
    assert_eq!(workflow["base_version"], json!(1), "{next}");
    assert_eq!(workflow["identity"]["target"], json!("peer"), "{next}");
    assert_eq!(workflow["identity"]["source"], json!("base"), "{next}");
    let specialties = workflow["specialties"].as_array().expect("specialty cards");
    assert_eq!(specialties.len(), 1, "{workflow}");
    let designer = &specialties[0];
    assert_eq!(designer["target"], json!("role:designer"), "{designer}");
    assert_eq!(designer["source"], json!("master_override"), "{designer}");
    assert_eq!(designer["base_version"], json!(1), "{designer}");
    assert_eq!(designer["revision"], json!(1), "{designer}");
    assert!(
        designer.get("template").is_none(),
        "specialty card must reference its source without embedding its text: {designer}"
    );

    let brief = &next["role_brief"];
    assert!(
        brief["role_task"]
            .as_str()
            .is_some_and(|text| text.contains("Draft the design proposal")),
        "role_task must carry the selected specialty template: {brief}"
    );
    assert!(
        brief["responsibilities"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == "Deliver the design draft")),
        "responsibilities must carry the selected specialty template: {brief}"
    );
    assert!(
        brief["next_action"]
            .as_str()
            .is_some_and(|text| text.contains("Publish the design draft")),
        "next_action must carry the selected specialty template: {brief}"
    );
    assert!(
        brief["completion_action"]
            .as_str()
            .is_some_and(|text| text.contains("Report the design result")),
        "completion_action must carry the selected specialty template: {brief}"
    );
    assert_no_static_workflow_operations(&next);
}

#[test]
fn master_preconfigured_role_template_appears_on_same_invocation_role_change() {
    let fixture = master_fixture();
    fixture.run_public(&["context"], THREAD_B);
    fixture.run_public(&["context", "--work-roles", "designer"], THREAD_B);
    let template = workflow_spec(
        "Translate the user request",
        "Keep terminology consistent",
        "Produce the translation",
        "Report the translated result",
    );
    let updated = fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "role:translator",
            "--workflow-template",
            template.as_str(),
        ],
        THREAD_A,
    );
    assert_eq!(
        updated["workflow_template_update"]["target"],
        json!("role:translator"),
        "{updated}"
    );
    let before = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(
        before["role_brief"]["workflow_template"]["specialties"],
        json!([]),
        "a preconfigured role template applies only to holders: {before}"
    );

    // The roles mutation invocation itself must project the new specialty,
    // not only the following `context` read.
    let switched = fixture.run_public(&["context", "--work-roles", "translator"], THREAD_B);
    assert_eq!(switched["work_roles"], json!(["translator"]));
    let cards = switched["role_brief"]["workflow_template"]["specialties"]
        .as_array()
        .expect("specialty cards");
    assert_eq!(cards.len(), 1, "{switched}");
    assert_eq!(cards[0]["target"], json!("role:translator"), "{switched}");
    assert_eq!(cards[0]["source"], json!("master_override"), "{switched}");
    assert!(
        cards[0].get("template").is_none(),
        "same-invocation specialty card must reference its source instead of embedding text: {switched}"
    );
    assert!(
        switched["role_brief"]["role_task"]
            .as_str()
            .is_some_and(|text| text.contains("Translate the user request")),
        "same-invocation role_brief must use the selected workflow: {switched}"
    );
    assert!(
        switched["role_brief"]["next_action"]
            .as_str()
            .is_some_and(|text| text.contains("Produce the translation")),
        "same-invocation next_action must use the selected workflow: {switched}"
    );
    assert_eq!(
        switched["next_actions"],
        json!([]),
        "a workflow role change must not fabricate a task action: {switched}"
    );
}

#[test]
fn multiple_role_templates_compose_and_role_changes_are_immediate() {
    let fixture = master_fixture();
    fixture.run_public(&["context", "--work-roles", "designer,reviewer"], THREAD_B);
    let designer = workflow_spec(
        "Draft the design proposal",
        "Deliver the design draft",
        "Publish the design draft",
        "Report the design result",
    );
    let reviewer = workflow_spec(
        "Review the delivered implementation",
        "Verify the acceptance conditions",
        "Accept or return the delivery",
        "Report the review result",
    );
    for (target, template) in [
        ("role:designer", designer.as_str()),
        ("role:reviewer", reviewer.as_str()),
    ] {
        fixture.run_public(
            &[
                "context",
                "--workflow-target",
                target,
                "--workflow-template",
                template,
            ],
            THREAD_A,
        );
    }

    let next = fixture.run_public(&["context"], THREAD_B);
    let cards = next["role_brief"]["workflow_template"]["specialties"]
        .as_array()
        .expect("specialty cards");
    let targets = cards
        .iter()
        .map(|card| card["target"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        vec!["role:designer", "role:reviewer"],
        "specialty cards must follow sorted current roles: {next}"
    );
    let responsibilities = next["role_brief"]["responsibilities"]
        .as_array()
        .expect("responsibilities");
    assert!(
        responsibilities
            .iter()
            .any(|item| item == "Deliver the design draft"),
        "{next}"
    );
    assert!(
        responsibilities
            .iter()
            .any(|item| item == "Verify the acceptance conditions"),
        "{next}"
    );

    let changed = fixture.run_public(&["context", "--work-roles", "reviewer"], THREAD_B);
    let cards = changed["role_brief"]["workflow_template"]["specialties"]
        .as_array()
        .expect("specialty cards");
    assert_eq!(cards.len(), 1, "{changed}");
    assert_eq!(cards[0]["target"], json!("role:reviewer"), "{changed}");
}

#[test]
fn workflow_template_reset_and_daemon_replay_are_public() {
    let mut fixture = master_fixture();
    fixture.run_public(&["context", "--work-roles", "designer"], THREAD_B);
    let template = workflow_spec(
        "Draft the design proposal",
        "Deliver the design draft",
        "Publish the design draft",
        "Report the design result",
    );
    let updated = fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "role:designer",
            "--workflow-template",
            template.as_str(),
        ],
        THREAD_A,
    );
    assert_eq!(updated["workflow_template_update"]["revision"], json!(1));

    fixture.restart_daemon();
    let replayed = fixture.run_public(&["context"], THREAD_B);
    let cards = replayed["role_brief"]["workflow_template"]["specialties"]
        .as_array()
        .expect("specialty cards");
    assert_eq!(cards.len(), 1, "{replayed}");
    assert_eq!(cards[0]["revision"], json!(1), "{replayed}");
    assert!(
        cards[0].get("template").is_none(),
        "replayed specialty card must reference its source instead of embedding text: {replayed}"
    );
    assert!(
        replayed["role_brief"]["role_task"]
            .as_str()
            .is_some_and(|text| text.contains("Draft the design proposal")),
        "replayed role_brief must retain the selected template text: {replayed}"
    );

    let reset = fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "role:designer",
            "--reset-workflow-template",
        ],
        THREAD_A,
    );
    assert_eq!(
        reset["workflow_template_update"]["target"],
        json!("role:designer")
    );
    let restored = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(
        restored["role_brief"]["workflow_template"]["identity"]["source"],
        json!("base"),
        "{restored}"
    );
    assert_eq!(
        restored["role_brief"]["workflow_template"]["identity"]["revision"],
        json!(0),
        "{restored}"
    );
    assert_eq!(
        restored["role_brief"]["workflow_template"]["specialties"],
        json!([]),
        "{restored}"
    );
}

#[test]
fn non_master_workflow_template_write_rejects_without_state_mutation() {
    let fixture = master_fixture();
    fixture.run_public(&["context"], THREAD_B);
    let template = workflow_spec(
        "Rewrite the peer workflow",
        "Ignore the authorized scope",
        "Act without master authority",
        "Report a forged result",
    );
    let rejected = fixture.command_public(
        &[
            "context",
            "--workflow-target",
            "peer",
            "--workflow-template",
            template.as_str(),
        ],
        THREAD_B,
    );
    let combined = format!(
        "{} {}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(!rejected.status.success(), "{combined}");
    assert!(
        combined.contains("WORKFLOW_TEMPLATE_MASTER_REQUIRED"),
        "{combined}"
    );

    for (label, snapshot) in [
        ("master", fixture.run_public(&["context"], THREAD_A)),
        ("peer", fixture.run_public(&["context"], THREAD_B)),
    ] {
        let workflow = &snapshot["role_brief"]["workflow_template"];
        assert_eq!(
            workflow["identity"]["source"],
            json!("base"),
            "{label}: rejected write must not mutate state: {workflow}"
        );
        assert_eq!(workflow["specialties"], json!([]), "{label}: {workflow}");
    }
}

#[test]
fn workflow_template_update_preserves_active_task_pins() {
    let fixture = master_fixture();
    let master = fixture.run_public(&["context"], THREAD_A);
    let peer = fixture.run_public(&["context"], THREAD_B);
    let master_name = master["name"].as_str().expect("master name").to_owned();
    let peer_name = peer["name"].as_str().expect("peer name").to_owned();
    fixture.run_public(&["context", "--work-roles", "designer"], THREAD_B);
    fixture.run_public(
        &[
            "board",
            "publish",
            "workflow-pins",
            "--title",
            "Workflow pins",
            "--description",
            "Template updates must not rewrite task pins",
            "--delivery-condition",
            "Pinned review passes",
            "--test-condition",
            "Public context contracts pass",
            "--executor",
            &peer_name,
            "--reviewer",
            &master_name,
        ],
        THREAD_A,
    );
    // Existing context bookkeeping syncs a newly published task to the stable
    // name's assignment. Let that settle first so the assertion below isolates
    // the workflow template update from ordinary name-resolution maintenance.
    fixture.run_public(&["context"], THREAD_A);
    let published = fixture.run_public(&["board", "show"], THREAD_A);
    let revision = task(&published, "workflow-pins")["revision"].to_string();
    fixture.run_public(
        &[
            "board",
            "dispatch",
            "workflow-pins",
            "--expected-revision",
            &revision,
        ],
        THREAD_A,
    );
    let invited = fixture.run_public(&["board", "show"], THREAD_A);
    let revision = task(&invited, "workflow-pins")["revision"].to_string();
    fixture.run_public(
        &[
            "board",
            "respond",
            "workflow-pins",
            "--accept",
            "--expected-revision",
            &revision,
        ],
        THREAD_B,
    );
    let accepted = fixture.run_public(&["board", "show"], THREAD_A);
    let revision = task(&accepted, "workflow-pins")["revision"].to_string();
    fixture.run_public(
        &[
            "board",
            "update",
            "workflow-pins",
            "--status",
            "working",
            "--expected-revision",
            &revision,
        ],
        THREAD_B,
    );
    let before = fixture.run_public(&["board", "show"], THREAD_A);
    let before_task = task(&before, "workflow-pins").clone();
    assert_eq!(before_task["status"], "working", "must test an active task");
    assert_eq!(before_task["owner"], peer["identity"]["worker_id"]);
    assert_eq!(before_task["executor_name"], json!(peer_name));
    assert_eq!(before_task["reviewer_name"], json!(master_name));

    let template = workflow_spec(
        "Draft the design proposal",
        "Deliver the design draft",
        "Publish the design draft",
        "Report the design result",
    );
    fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "role:designer",
            "--workflow-template",
            template.as_str(),
        ],
        THREAD_A,
    );
    let after = fixture.run_public(&["board", "show"], THREAD_A);
    let after_task = task(&after, "workflow-pins");
    assert_eq!(
        after_task["executor_name"],
        json!(peer_name),
        "{after_task}"
    );
    assert_eq!(
        after_task["reviewer_name"],
        json!(master_name),
        "{after_task}"
    );
    assert_eq!(after_task["revision"], before_task["revision"]);
    assert_eq!(after_task["status"], before_task["status"]);
    assert_eq!(after_task["owner"], before_task["owner"]);
}

#[test]
fn workflow_template_flag_conflicts_are_rejected_before_identity_side_effects() {
    let fixture = AppFixture::new();
    let template = workflow_spec(
        "Draft the design proposal",
        "Deliver the design draft",
        "Publish the design draft",
        "Report the design result",
    );
    for args in [
        vec!["context", "--workflow-target", "master"],
        vec!["context", "--workflow-template", template.as_str()],
        vec![
            "context",
            "--workflow-target",
            "master",
            "--workflow-template",
            template.as_str(),
            "--reset-workflow-template",
        ],
        vec![
            "context",
            "--workflow-target",
            "master",
            "--workflow-template",
            template.as_str(),
            "--work-roles",
            "designer",
        ],
        vec![
            "context",
            "--workflow-target",
            "master",
            "--workflow-template",
            template.as_str(),
            "--query",
        ],
    ] {
        let rejected = fixture.command_public(&args, THREAD_A);
        assert!(
            !rejected.status.success(),
            "conflicting workflow flags must fail: {args:?}: {}{}",
            String::from_utf8_lossy(&rejected.stdout),
            String::from_utf8_lossy(&rejected.stderr)
        );
    }
    let invalid_json = fixture.command_public(
        &[
            "context",
            "--workflow-target",
            "master",
            "--workflow-template",
            "{not json",
        ],
        THREAD_A,
    );
    let combined = format!(
        "{} {}",
        String::from_utf8_lossy(&invalid_json.stdout),
        String::from_utf8_lossy(&invalid_json.stderr)
    );
    assert!(!invalid_json.status.success(), "{combined}");
    assert!(
        combined.contains("CONTEXT_WORKFLOW_TEMPLATE_INVALID"),
        "{combined}"
    );
    let identities = fixture.host_state.join("identities");
    let created_identity = identities.exists()
        && std::fs::read_dir(&identities)
            .expect("read isolated identities dir")
            .next()
            .is_some();
    assert!(
        !created_identity,
        "rejected workflow flags must not create an identity: {identities:?}"
    );
}

fn role_cards(snapshot: &Value) -> Vec<(String, String)> {
    let mut cards: Vec<(String, String)> = snapshot["role_brief"]["workflow_template"]
        ["specialties"]
        .as_array()
        .expect("workflow_template.specialties is an array")
        .iter()
        .map(|card| {
            (
                card["target"].as_str().unwrap_or("").to_owned(),
                card["source"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect();
    cards.sort();
    cards
}

fn role_projection_text(snapshot: &Value) -> String {
    let brief = &snapshot["role_brief"];
    let mut parts: Vec<String> = Vec::new();
    for field in ["role_task", "next_action", "completion_action"] {
        if let Some(text) = brief[field].as_str() {
            if !text.is_empty() {
                parts.push(text.to_owned());
            }
        }
    }
    if let Some(responsibilities) = brief["responsibilities"].as_array() {
        for item in responsibilities {
            if let Some(text) = item.as_str() {
                if !text.is_empty() {
                    parts.push(text.to_owned());
                }
            }
        }
    }
    parts.join("\n")
}

/// Match a semantic marker in the projected role text without depending on the
/// template's letter case. The assertions below check a few delivered duties,
/// not a full copy of the template body.
fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[test]
fn builtin_specialty_roles_project_on_registration_and_repeated_context() {
    let fixture = AppFixture::new();

    let cases = [
        (
            "worker",
            "worker",
            ["task contract", "execute the actual work", "expand scope"],
        ),
        (
            "patrol",
            "patrol",
            [
                "temporary development or scheduling dispatch",
                "Jenny",
                "project memory",
            ],
        ),
        (
            "memory",
            "memory",
            [
                "reusable facts",
                "deduplicate",
                "take over product development",
            ],
        ),
        (
            "architect",
            "architect",
            [
                "effective requirements",
                "single implementation owner",
                "planning, implementation, and final review separate",
            ],
        ),
    ];

    for (role, target, phrases) in cases {
        let snapshot = fixture.run_public(&["context", "--work-roles", role], THREAD_A);
        assert_eq!(
            snapshot["work_roles"],
            json!([role]),
            "{role}: work_roles should preserve the set value: {snapshot}"
        );
        assert_eq!(
            role_cards(&snapshot),
            vec![(target.to_owned(), "builtin_default".to_owned())],
            "{role}: only the canonical default specialty card should project: {snapshot}"
        );
        let text = role_projection_text(&snapshot);
        for phrase in phrases {
            assert!(
                contains_ci(&text, phrase),
                "{role}: role_brief text must include {phrase:?}: {text}"
            );
        }
        assert!(
            text.contains("appsdk goal status --json") && text.contains("goal_path"),
            "{role}: role_brief must explain the goal document entry without running it: {text}"
        );
    }

    let last = fixture.run_public(&["context", "--work-roles", "architect"], THREAD_A);
    let repeated = fixture.run_public(&["context"], THREAD_A);
    assert_eq!(
        repeated["role_brief"], last["role_brief"],
        "repeated context without role changes must not alter the role_brief: {repeated}"
    );
    assert_eq!(
        role_cards(&repeated),
        vec![("architect".to_owned(), "builtin_default".to_owned())],
        "repeated context must keep the same built-in specialty card: {repeated}"
    );
}

#[test]
fn explicit_worker_peer_executes_directly_without_ordinary_peer_orchestration() {
    let fixture = master_fixture();

    // A peer with no execution specialty keeps the ordinary long-horizon
    // orchestration contract.
    let ordinary = fixture.run_public(&["context"], THREAD_B);
    let ordinary_text = role_projection_text(&ordinary);
    assert!(
        contains_ci(
            &ordinary_text,
            "prefer assigning code implementation to subagents"
        ),
        "ordinary peer must keep the orchestrator workflow: {ordinary_text}"
    );
    assert!(
        contains_ci(
            &ordinary_text,
            "split the accepted goal into non-overlapping bounded scopes"
        ),
        "ordinary peer must keep the delegated-goal dispatch duty: {ordinary_text}"
    );

    // The same peer with an explicit worker specialty performs the bounded work
    // directly and must not inherit the unconditional dispatch instruction.
    let worker = fixture.run_public(&["context", "--work-roles", "worker"], THREAD_B);
    let worker_text = role_projection_text(&worker);
    assert!(
        !contains_ci(
            &worker_text,
            "split the accepted goal into non-overlapping bounded scopes"
        ),
        "worker specialty must not be told to dispatch subworkers first: {worker_text}"
    );
    assert!(
        !contains_ci(&worker_text, "prefer assigning code implementation to subagents"),
        "worker specialty must not inherit the ordinary peer's delegation-first identity: {worker_text}"
    );
    assert!(
        contains_ci(&worker_text, "execute the actual work"),
        "worker specialty must keep the direct execution duty: {worker_text}"
    );
    // The work specialty does not change identity, authority or goal level.
    for field in ["role", "authority", "derivation", "goal_level"] {
        assert_eq!(
            worker["role_brief"][field], ordinary["role_brief"][field],
            "worker specialty must not change role_brief.{field}: {worker}"
        );
    }
}

#[test]
fn fixed_and_design_specialties_drop_orchestration_and_preserve_separation() {
    let fixture = master_fixture();

    // Fixed-duty and design specialties must not inherit the ordinary peer's
    // unconditional dispatch and delegation-first instruction.
    for role in ["patrol", "memory", "architect"] {
        let snapshot = fixture.run_public(&["context", "--work-roles", role], THREAD_B);
        let text = role_projection_text(&snapshot);
        assert!(
            !contains_ci(
                &text,
                "split the accepted goal into non-overlapping bounded scopes"
            ),
            "{role}: fixed or design specialty must not inherit dispatch duty: {text}"
        );
        assert!(
            !contains_ci(&text, "prefer assigning code implementation to subagents"),
            "{role}: fixed or design specialty must not inherit delegation-first identity: {text}"
        );
    }

    // Mixing the architect with the worker keeps both work texts and keeps
    // planning, implementation and final review separated.
    let mixed = fixture.run_public(&["context", "--work-roles", "architect,worker"], THREAD_B);
    let mixed_text = role_projection_text(&mixed);
    assert!(
        !contains_ci(&mixed_text, "temporary development or scheduling dispatch"),
        "patrol specialization boundaries are unrelated to this mix"
    );
    for phrase in [
        "planning, implementation, and final review separate",
        "produce a detailed architecture or plan file on disk",
        "execute the actual work",
    ] {
        assert!(
            contains_ci(&mixed_text, phrase),
            "mixed architect/worker must keep {phrase:?}: {mixed_text}"
        );
    }
    assert!(
        !contains_ci(
            &mixed_text,
            "split the accepted goal into non-overlapping bounded scopes"
        ),
        "mixed specialty roles must not restore the ordinary peer dispatch duty: {mixed_text}"
    );
}

#[test]
fn builtin_aliases_are_canonical_and_exact_overrides_are_not_swallowed() {
    let fixture = master_fixture();
    fixture.run_public(&["context"], THREAD_B);

    let aliases = fixture.run_public(
        &[
            "context",
            "--work-roles",
            "worker,executor,巡察者,记忆 agent,架构师",
        ],
        THREAD_B,
    );
    assert_eq!(
        aliases["work_roles"],
        json!(["executor", "worker", "巡察者", "架构师", "记忆 agent"]),
        "stored specialty values must not be rewritten by alias matching: {aliases}"
    );
    let mut targets: Vec<String> = role_cards(&aliases)
        .iter()
        .map(|(target, _)| target.clone())
        .collect();
    targets.sort();
    assert_eq!(
        targets,
        vec![
            "architect".to_owned(),
            "memory".to_owned(),
            "patrol".to_owned(),
            "worker".to_owned()
        ],
        "同义角色必须按 canonical 去重，且不能出现多个 worker 正文: {aliases:?}"
    );

    let custom_worker = workflow_spec(
        "Custom executor identity task",
        "Custom executor responsibility",
        "Custom executor next action",
        "Custom executor completion",
    );
    let custom_executor = workflow_spec(
        "Custom designer identity task",
        "Custom designer responsibility",
        "Custom designer next action",
        "Custom designer completion",
    );
    fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "role:worker",
            "--workflow-template",
            &custom_worker,
        ],
        THREAD_A,
    );
    fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "role:executor",
            "--workflow-template",
            &custom_executor,
        ],
        THREAD_A,
    );

    let exact = fixture.run_public(
        &["context", "--work-roles", "worker,executor,巡察者"],
        THREAD_B,
    );
    assert_eq!(
        exact["work_roles"],
        json!(["executor", "worker", "巡察者"]),
        "exact stored role values must remain exact: {exact}"
    );
    assert_eq!(
        role_cards(&exact),
        vec![
            ("patrol".to_owned(), "builtin_default".to_owned()),
            ("role:executor".to_owned(), "master_override".to_owned()),
            ("role:worker".to_owned(), "master_override".to_owned()),
        ],
        "exact overrides must win over canonical defaults and remain separate cards: {exact}"
    );
    let text = role_projection_text(&exact);
    assert!(
        text.contains("Custom executor identity task")
            && text.contains("Custom designer identity task"),
        "exact override text must be projected: {text}"
    );
    assert!(
        text.contains("Jenny") || text.contains("patrol"),
        "built-in default card for patrol must still appear alongside exact overrides: {text}"
    );
}

#[test]
fn specialty_reset_clear_and_role_labels_keep_authority_and_side_effects() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = master_fixture();
    let baseline = fixture.run_public(&["context"], THREAD_B);

    let worker = fixture.run_public(&["context", "--work-roles", "worker"], THREAD_B);
    assert_eq!(
        role_cards(&worker),
        vec![("worker".to_owned(), "builtin_default".to_owned())]
    );

    let custom_worker = workflow_spec(
        "Custom reset target task",
        "Custom reset target responsibility",
        "Custom reset target next action",
        "Custom reset target completion",
    );
    fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "role:worker",
            "--workflow-template",
            &custom_worker,
        ],
        THREAD_A,
    );
    // The override targets the worker role that the peer holds. Observe it
    // through the peer's next projection, not the master's own role labels.
    let overridden = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(
        role_cards(&overridden),
        vec![("role:worker".to_owned(), "master_override".to_owned())],
        "exact override replaces built-in default for that target: {overridden}"
    );

    let reset_receipt = fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "role:worker",
            "--reset-workflow-template",
        ],
        THREAD_A,
    );
    assert_eq!(
        reset_receipt["workflow_template_update"]["outcome"],
        json!("reset"),
        "reset must clear exact override for role:worker: {reset_receipt}"
    );
    let after_reset = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(
        role_cards(&after_reset),
        vec![("worker".to_owned(), "builtin_default".to_owned())],
        "reset role with built-in specialty must return to the default card: {after_reset}"
    );
    let text = role_projection_text(&after_reset);
    assert!(
        !text.contains("Custom reset target task"),
        "reset must not retain exact override text: {text}"
    );

    let translator = fixture.run_public(&["context", "--work-roles", "translator"], THREAD_B);
    assert_eq!(
        translator["work_roles"],
        json!(["translator"]),
        "unknown specialty is stored as given: {translator}"
    );
    let custom_translator = workflow_spec(
        "Custom unknown target task",
        "Custom unknown target responsibility",
        "Custom unknown target next action",
        "Custom unknown target completion",
    );
    fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "role:translator",
            "--workflow-template",
            &custom_translator,
        ],
        THREAD_A,
    );
    let unknown = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(
        role_cards(&unknown),
        vec![("role:translator".to_owned(), "master_override".to_owned())],
        "unknown exact override remains visible: {unknown}"
    );
    let reset_unknown = fixture.run_public(
        &[
            "context",
            "--workflow-target",
            "role:translator",
            "--reset-workflow-template",
        ],
        THREAD_A,
    );
    assert_eq!(
        reset_unknown["workflow_template_update"]["outcome"],
        json!("reset"),
        "reset unknown specialty must return to no specialty card: {reset_unknown}"
    );
    let after_reset_unknown = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(
        role_cards(&after_reset_unknown),
        vec![] as Vec<(String, String)>,
        "reset unknown specialty must remove the specialty card: {after_reset_unknown}"
    );
    assert!(
        !role_projection_text(&after_reset_unknown).contains("Custom unknown target task"),
        "reset unknown specialty must remove its text: {after_reset_unknown}"
    );

    let labeled = fixture.run_public(&["context", "--work-roles", "master,ceo"], THREAD_B);
    assert_eq!(
        labeled["identity"]["role"], baseline["identity"]["role"],
        "work-role labels must not change the wire identity: {labeled}"
    );
    assert_eq!(
        labeled["role_brief"]["role"], baseline["role_brief"]["role"],
        "work-role labels must not grant master/CEO role_brief: {labeled}"
    );
    assert_eq!(
        labeled["role_brief"]["authority"], baseline["role_brief"]["authority"],
        "work-role labels must not change authority fields: {labeled}"
    );
    let denied = fixture.command_public(
        &[
            "context",
            "--workflow-target",
            "role:worker",
            "--workflow-template",
            &custom_worker,
        ],
        THREAD_B,
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&denied.stdout),
        String::from_utf8_lossy(&denied.stderr)
    );
    assert!(
        !denied.status.success(),
        "a work-role label alone must not authorize workflow template mutation: {combined}"
    );
    assert!(
        combined.contains("WORKFLOW_TEMPLATE_MASTER_REQUIRED"),
        "mutation by non-master must keep the original error: {combined}"
    );

    let before = fixture.current_state();
    let log_path = fixture.root.join("appsdk-calls.log");
    let appsdk = fixture.path_prepend.join("appsdk");
    std::fs::write(
        &appsdk,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
            log_path.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&appsdk).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&appsdk, permissions).unwrap();

    let repeated_label = fixture.run_public(&["context"], THREAD_B);
    let after = fixture.current_state();
    for field in [
        "tasks",
        "notification_subscriptions",
        "workflow_templates",
        "name_work_roles",
    ] {
        assert_eq!(
            before[field], after[field],
            "context must not create task/subscription/template/role state: {field}"
        );
    }
    assert_eq!(
        repeated_label["role_brief"], labeled["role_brief"],
        "repeated context must not alter projection: {repeated_label}"
    );
    assert!(
        !log_path.exists() || std::fs::metadata(&log_path).unwrap().len() == 0,
        "context must not invoke appsdk goal or any target-side command: {log_path:?}"
    );

    let cleared = fixture.run_public(&["context", "--work-roles", ""], THREAD_B);
    assert_eq!(
        cleared["work_roles"],
        json!([]),
        "clearing work roles must return an empty set: {cleared}"
    );
    assert_eq!(
        role_cards(&cleared),
        vec![] as Vec<(String, String)>,
        "cleared roles must remove specialty projections: {cleared}"
    );
    let after_clear = fixture.run_public(&["context"], THREAD_B);
    assert_eq!(
        after_clear["work_roles"],
        json!([]),
        "repeated context after clearing must keep an empty role set: {after_clear}"
    );
    assert_eq!(
        role_cards(&after_clear),
        vec![] as Vec<(String, String)>,
        "repeated context after clearing must keep no specialty card: {after_clear}"
    );
}
