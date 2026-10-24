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
        for field in [
            "role_task",
            "responsibilities",
            "next_action",
            "completion_action",
        ] {
            let value = &identity["template"][field];
            assert!(
                !value.is_null(),
                "{label}: identity template must expose {field}: {identity}"
            );
            if field == "responsibilities" {
                assert!(
                    !value.as_array().expect("responsibilities array").is_empty(),
                    "{label}: identity responsibilities must be nonempty: {identity}"
                );
            } else {
                assert!(
                    !value.as_str().expect("template text").trim().is_empty(),
                    "{label}: identity template {field} must be nonempty: {identity}"
                );
            }
        }
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
    assert_eq!(
        designer["template"]["role_task"],
        json!("Draft the design proposal"),
        "{designer}"
    );
    assert_eq!(
        designer["template"]["next_action"],
        json!("Publish the design draft"),
        "{designer}"
    );
    assert_eq!(
        designer["template"]["completion_action"],
        json!("Report the design result"),
        "{designer}"
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
    let operations = next["operations"].as_array().expect("operations");
    assert!(
        operations.iter().any(|operation| {
            operation["kind"] == "next_action" && operation["action"] == brief["next_action"]
        }),
        "operations must derive the final next_action: {operations:?}"
    );
    assert!(
        operations.iter().any(|operation| {
            operation["kind"] == "responsibility"
                && operation["action"] == "Deliver the design draft"
        }),
        "operations must derive the selected specialty responsibilities: {operations:?}"
    );
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
    assert_eq!(
        cards[0]["template"]["role_task"],
        json!("Translate the user request"),
        "{switched}"
    );
    assert!(
        switched["next_actions"]
            .as_array()
            .is_some_and(|actions| actions
                .iter()
                .any(|action| action == &switched["role_brief"]["next_action"])),
        "same-invocation next_actions must use the selected workflow, not the pre-mutation flow"
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
    assert_eq!(
        cards[0]["template"]["role_task"],
        json!("Draft the design proposal"),
        "{replayed}"
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
