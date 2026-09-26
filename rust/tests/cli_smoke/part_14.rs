#[test]
fn guidance_plan_update_is_evidence_bound_idempotent_and_drift_safe() {
    let root = temp_root("guidance-update");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "mode": "develop",
            "goal_id": "goal-change-me",
            "task_id": "task-2",
            "module_id": "app-core",
            "objective": "test plan updates",
            "scope_paths": ["playground/experiments/**"],
            "steps": [
                {"step_id":"step-1","node_id":"requirements","action":"analyze","owner":"app-core","expected_evidence":["requirements"]},
                {"step_id":"step-2","node_id":"map_check","action":"bind maps","owner":"app-core","expected_evidence":["function-map-binding","verification-map-binding"]}
            ]
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let planned = run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-2",
        "--input",
        "plan.json",
    ]);
    assert!(
        planned.status.success(),
        "{}",
        String::from_utf8_lossy(&planned.stderr)
    );
    assert!(root
        .join(".appsdk-control/guidance/task-2/plan.json")
        .is_file());

    fs::write(
        root.join("result.json"),
        r#"{"schema_version":1,"event_id":"event-1","step_id":"step-1","result":"pass","observations":["closed"],"evidence":[]}
"#,
    )
    .unwrap();
    let missing = run(&[
        "guide",
        "update",
        root_text,
        "--task",
        "task-2",
        "--input",
        "result.json",
    ]);
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("GUIDANCE_PASS_REQUIRES_EVIDENCE:step-1")
    );

    fs::write(
        root.join("result.json"),
        r#"{"schema_version":1,"event_id":"event-1","step_id":"step-1","result":"pass","observations":["closed"],"evidence":["requirements-1"]}
"#,
    )
    .unwrap();
    let first = run(&[
        "guide",
        "update",
        root_text,
        "--task",
        "task-2",
        "--input",
        "result.json",
    ]);
    assert!(first.status.success());
    let duplicate = run(&[
        "guide",
        "update",
        root_text,
        "--task",
        "task-2",
        "--input",
        "result.json",
    ]);
    assert!(duplicate.status.success());
    let duplicate_json: Value = serde_json::from_slice(&duplicate.stdout).unwrap();
    assert_eq!(duplicate_json["idempotent"], true);

    fs::write(
        root.join("result.json"),
        r#"{"schema_version":1,"event_id":"event-1","step_id":"step-1","result":"pass","observations":["changed"],"evidence":["requirements-1"]}
"#,
    )
    .unwrap();
    let conflict = run(&[
        "guide",
        "update",
        root_text,
        "--task",
        "task-2",
        "--input",
        "result.json",
    ]);
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("GUIDANCE_EVENT_CONFLICT:event-1"));

    let next = run(&["guide", "next", root_text, "--task", "task-2"]);
    assert!(next.status.success());
    let next_json: Value = serde_json::from_slice(&next.stdout).unwrap();
    assert_eq!(next_json["next"]["node_id"], "map_check");

    let goal_file = root.join(".appsdk/goal.json");
    let mut goal: Value = serde_json::from_str(&fs::read_to_string(&goal_file).unwrap()).unwrap();
    goal["understood_objective"] = Value::String("drifted objective".into());
    fs::write(
        &goal_file,
        serde_json::to_string_pretty(&goal).unwrap() + "\n",
    )
    .unwrap();
    fs::write(
        root.join("result.json"),
        r#"{"schema_version":1,"event_id":"event-1","step_id":"step-1","result":"pass","observations":["closed"],"evidence":["requirements-1"]}
"#,
    )
    .unwrap();
    let replay_after_drift = run(&[
        "guide",
        "update",
        root_text,
        "--task",
        "task-2",
        "--input",
        "result.json",
    ]);
    assert!(replay_after_drift.status.success());
    let replay_json: Value = serde_json::from_slice(&replay_after_drift.stdout).unwrap();
    assert_eq!(replay_json["idempotent"], true);

    fs::write(
        root.join("result.json"),
        r#"{"schema_version":1,"event_id":"event-2","step_id":"step-2","result":"pass","observations":[],"evidence":["architecture-1"]}
"#,
    )
    .unwrap();
    let drift = run(&[
        "guide",
        "update",
        root_text,
        "--task",
        "task-2",
        "--input",
        "result.json",
    ]);
    assert!(!drift.status.success());
    assert!(String::from_utf8_lossy(&drift.stderr).contains("GUIDANCE_CONTEXT_DRIFT:goal"));

    let status = run(&["guide", "next", root_text, "--task", "task-2"]);
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        status_json["next"]["revision_reason"],
        "rule_context_changed"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_tour_review_requires_node_content_before_flow_update() {
    let root = temp_root("guidance-tour-review");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);
    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "mode": "develop",
            "goal_id": "goal-change-me",
            "task_id": "tour-task",
            "module_id": "app-core",
            "objective": "tour review ordering",
            "scope_paths": ["playground/experiments/**"],
            "steps": [
                {"step_id":"step-1","node_id":"requirements","action":"inspect","owner":"app-core","expected_evidence":["requirements"]},
                {"step_id":"step-2","node_id":"map_check","action":"inspect","owner":"app-core","expected_evidence":["function-map-binding","verification-map-binding"]}
            ]
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "plan.json"
    ])
    .status
    .success());

    fs::write(
        root.join("tour.json"),
        r#"{"schema_version":1,"tour_id":"tour-1","selected_path":["requirements","map_check"]}
"#,
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "tour",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "tour.json"
    ])
    .status
    .success());

    fs::write(
        root.join("flow-review.json"),
        r#"{"schema_version":1,"review_id":"flow-before-nodes","stage":"flow_review","flow_update":{"order":["requirements","map_check"],"edges":[{"from":"requirements","to":"map_check"}],"rules":["keep-adjacent"]}}
"#,
    )
    .unwrap();
    let blocked = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "flow-review.json",
    ]);
    assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr)
        .contains("GUIDANCE_FLOW_REVIEW_REQUIRES_NODE_APPROVAL"));

    fs::write(
        root.join("node-one.json"),
        r#"{"schema_version":1,"review_id":"node-1","stage":"node_review","node_updates":[{"node_id":"requirements","verdict":"accept","content":"confirmed requirements"}]}
"#,
    )
    .unwrap();
    let first = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "node-one.json",
    ]);
    assert!(first.status.success());
    let first_json: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first_json["node_review_complete"], false);

    fs::write(
        root.join("node-two.json"),
        r#"{"schema_version":1,"review_id":"node-2","stage":"node_review","node_updates":[{"node_id":"map_check","verdict":"approved","content":"confirmed maps"}]}
"#,
    )
    .unwrap();
    let second = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "node-two.json",
    ]);
    assert!(second.status.success());
    let second_json: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(second_json["node_review_complete"], true);
    assert_eq!(second_json["flow_review_allowed"], true);

    fs::write(
        root.join("empty-flow.json"),
        r#"{"schema_version":1,"review_id":"empty-flow","stage":"flow_review"}
"#,
    )
    .unwrap();
    let empty_flow = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "empty-flow.json",
    ]);
    assert!(!empty_flow.status.success());
    assert!(String::from_utf8_lossy(&empty_flow.stderr).contains("GUIDANCE_FLOW_UPDATE_REQUIRED"));

    fs::write(
        root.join("rejected-flow.json"),
        r#"{"schema_version":1,"review_id":"flow-rejected","stage":"flow_review","verdict":"reject","flow_update":{"order":["requirements","map_check"],"edges":[{"from":"requirements","to":"map_check"}],"rules":["keep-adjacent"]}}
"#,
    )
    .unwrap();
    let rejected_flow = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "rejected-flow.json",
    ]);
    assert!(rejected_flow.status.success());
    let rejected_json: Value = serde_json::from_slice(&rejected_flow.stdout).unwrap();
    assert_eq!(rejected_json["verdict"], "rejected");
    assert_eq!(rejected_json["flow_revision"], Value::Null);
    assert!(rejected_json["next"]
        .as_str()
        .unwrap()
        .contains("submit flow_review again"));

    let flow = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "flow-review.json",
    ]);
    assert!(
        flow.status.success(),
        "{}",
        String::from_utf8_lossy(&flow.stderr)
    );
    let flow_json: Value = serde_json::from_slice(&flow.stdout).unwrap();
    assert_eq!(flow_json["stage"], "flow_review");
    assert_eq!(flow_json["active_revision_changed"], false);
    let events =
        fs::read_to_string(root.join(".appsdk-control/guidance/tour-task/events.jsonl")).unwrap();
    let flow_line = events
        .lines()
        .find(|line| line.contains("\"stage\":\"flow_review\""))
        .unwrap();
    assert!(flow_line.contains("node-requirements-"));
    assert!(flow_line.contains("node-map_check-"));

    fs::write(
        root.join("node-revision.json"),
        r#"{"schema_version":1,"review_id":"node-3","stage":"node_review","node_updates":[{"node_id":"requirements","verdict":"accept","content":"reconfirmed requirements"}]}
"#,
    )
    .unwrap();
    let revised_node = run(&[
        "guide",
        "review",
        root_text,
        "--task",
        "tour-task",
        "--input",
        "node-revision.json",
    ]);
    assert!(revised_node.status.success());
    let status = run(&["guide", "status", root_text, "--task", "tour-task"]);
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["tour_review"]["stage"], "flow_review");
    assert_eq!(status_json["tour_review"]["flow_review_complete"], false);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_memory_index_query_review_and_compact_are_layered() {
    let root = temp_root("project-memory");
    fs::create_dir_all(&root).unwrap();
    let memory_home = temp_root("project-memory-home");
    fs::create_dir_all(&memory_home).unwrap();
    let root_text = root.to_str().unwrap();
    let home_text = memory_home.to_str().unwrap();
    let anchor = run_memory(
        &root,
        &[
            "entry",
            "--id",
            "plan-root",
            "--category",
            "plan",
            "--text",
            "stable project anchor",
            "--importance",
            "95",
            "--tag",
            "owner",
        ],
        &memory_home,
    );
    assert!(
        anchor.status.success(),
        "{}",
        String::from_utf8_lossy(&anchor.stderr)
    );
    let fact = run_memory(
        &root,
        &[
            "entry",
            "--id",
            "fact-one",
            "--category",
            "knowledge",
            "--text",
            "SQLite stores the rebuildable index",
            "--tag",
            "storage",
        ],
        &memory_home,
    );
    assert!(
        fact.status.success(),
        "{}",
        String::from_utf8_lossy(&fact.stderr)
    );
    let queried = run_memory(&root, &["query", "SQLite"], &memory_home);
    assert!(queried.status.success());
    let queried_json: Value = serde_json::from_slice(&queried.stdout).unwrap();
    assert_eq!(queried_json["semantic_backend"]["status"], "candidate-only");
    assert!(queried_json["keyword_matches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == "fact-one"));
    let inspected = run_memory(&root, &["verify"], &memory_home);
    assert!(inspected.status.success());
    let inspected_json: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(inspected_json["ok"], true);

    fs::create_dir_all(memory_home.join("collab/runs/run-memory")).unwrap();
    fs::write(
        memory_home.join("collab/runs/run-memory/notes.jsonl"),
        r#"{"record_type":"lesson","memory":{"id":"lesson-one","category":"lesson","content":"verified review ordering","tags":["review"]}}
"#,
    )
    .unwrap();
    let reviewed = run_memory(&root, &["review", "--run", "run-memory"], &memory_home);
    assert!(
        reviewed.status.success(),
        "{}",
        String::from_utf8_lossy(&reviewed.stderr)
    );
    let reviewed_json: Value = serde_json::from_slice(&reviewed.stdout).unwrap();
    assert_eq!(reviewed_json["checked"], true);
    let duplicate = run_memory(
        &root,
        &[
            "entry",
            "--id",
            "lesson-one",
            "--category",
            "lesson",
            "--text",
            "verified review ordering",
            "--tag",
            "ordering",
        ],
        &memory_home,
    );
    assert!(duplicate.status.success());
    let compacted = run_memory(&root, &["compact"], &memory_home);
    assert!(
        compacted.status.success(),
        "{}",
        String::from_utf8_lossy(&compacted.stderr)
    );
    let lesson = run_memory(&root, &["get", "lesson-one"], &memory_home);
    let lesson_json: Value = serde_json::from_slice(&lesson.stdout).unwrap();
    let expected_ref = format!(
        "{}/runs/run-memory/notes.jsonl#1",
        memory_home.join("collab").display()
    );
    assert!(
        lesson_json["matches"][0]["source_refs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == &Value::String(expected_ref.clone())),
        "source_refs must identify the configured COLLAB_STATE_DIR run notes: {lesson_json}"
    );
    let tags = lesson_json["matches"][0]["tags"].as_array().unwrap();
    assert!(tags.iter().any(|tag| tag == "review"));
    assert!(tags.iter().any(|tag| tag == "ordering"));
    assert!(!home_text.is_empty() && !root_text.is_empty());
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn project_memory_edges_and_latest_compaction_are_explicit() {
    let root = temp_root("project-memory-edges");
    let caller = temp_root("project-memory-edge-caller");
    let memory_home = temp_root("project-memory-edge-home");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&caller).unwrap();
    fs::create_dir_all(&memory_home).unwrap();
    fs::create_dir_all(root.join("memory")).unwrap();
    fs::write(
        root.join("memory/knowledge.jsonl"),
        concat!(
            r#"{"id":"anchor-edge","category":"knowledge","title":"Anchor","content":"stable node anchor","tags":["stable"],"source_refs":["guide/node"],"related_ids":["lesson-edge"],"semantic_relations":[{"to_id":"lesson-edge","type":"similar_lesson","score":0.91,"model_revision":"wemm-test-v1"}],"importance":95,"memory_level":1,"review_status":"reviewed","review_evidence":["review/anchor-edge"]}"#,
            "\n",
            r#"{"id":"latest-entry","category":"knowledge","content":"old content","tags":["old"],"source_refs":["source/old"],"updated_at":"2026-01-01T00:00:00Z"}"#,
            "\n",
            r#"{"id":"latest-entry","category":"knowledge","content":"new content","tags":["new"],"source_refs":["source/new"],"updated_at":"2026-01-02T00:00:00Z"}"#,
            "\n",
            r#"{"id":"lesson-edge","category":"lesson","content":"verified historical lesson","tags":["lesson"],"source_refs":["review/1"]}"#,
            "\n"
        ),
    )
    .unwrap();

    let indexed = run_memory(&root, &["index"], &memory_home);
    assert!(
        indexed.status.success(),
        "{}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    let root_text = root.to_str().unwrap();
    let queried = run_memory(&caller, &["query", "stable", root_text], &memory_home);
    assert!(
        queried.status.success(),
        "{}",
        String::from_utf8_lossy(&queried.stderr)
    );
    let queried_json: Value = serde_json::from_slice(&queried.stdout).unwrap();
    assert!(queried_json["anchors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == "anchor-edge"));
    assert!(queried_json["declared_related"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| edge["from_id"] == "anchor-edge" && edge["to_id"] == "lesson-edge"));
    assert!(queried_json["semantic_related"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| edge["type"] == "similar_lesson" && edge["model_revision"] == "wemm-test-v1"));

    let inspected = run_memory(&caller, &["get", "anchor-edge", root_text], &memory_home);
    assert!(inspected.status.success());
    let inspected_json: Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(inspected_json["matches"][0]["scope"], "project");

    let source_before = fs::read_to_string(root.join("memory/knowledge.jsonl")).unwrap();
    let compacted = run_memory(&root, &["compact"], &memory_home);
    assert!(
        compacted.status.success(),
        "{}",
        String::from_utf8_lossy(&compacted.stderr)
    );
    let source_after = fs::read_to_string(root.join("memory/knowledge.jsonl")).unwrap();
    assert_eq!(
        source_after, source_before,
        "compact must retain raw event history"
    );
    let latest = run_memory(&root, &["get", "latest-entry"], &memory_home);
    assert!(latest.status.success());
    let latest: Value = serde_json::from_slice(&latest.stdout).unwrap();
    let latest = &latest["matches"][0];
    assert_eq!(latest["content"], "new content");
    assert!(latest["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tag| tag == "old"));
    assert!(latest["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tag| tag == "new"));
    assert!(latest["source_refs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source == "source/old"));
    assert!(latest["source_refs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source == "source/new"));

    let empty = run_memory(&root, &["query", "   "], &memory_home);
    assert!(!empty.status.success());
    assert!(String::from_utf8_lossy(&empty.stderr).contains("MEMORY_QUERY_EMPTY"));
    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(caller).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn project_memory_updates_follow_current_version_and_verify_source_drift() {
    let root = temp_root("project-memory-versioning");
    let memory_home = temp_root("project-memory-versioning-home");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&memory_home).unwrap();

    for (content, tag) in [("A", "a"), ("B", "b"), ("A", "c")] {
        let result = run_memory(
            &root,
            &[
                "entry",
                "--id",
                "cycle",
                "--category",
                "knowledge",
                "--text",
                content,
                "--tag",
                tag,
            ],
            &memory_home,
        );
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let source = fs::read_to_string(root.join("memory/knowledge.jsonl")).unwrap();
    assert_eq!(
        source.lines().count(),
        3,
        "A -> B -> A must retain three events"
    );
    let current = run_memory(&root, &["get", "cycle"], &memory_home);
    assert!(current.status.success());
    let current: Value = serde_json::from_slice(&current.stdout).unwrap();
    assert_eq!(current["matches"][0]["content"], "A");
    for tag in ["a", "b", "c"] {
        assert!(current["matches"][0]["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == tag));
    }
    let wrong_category = run_memory(
        &root,
        &[
            "entry",
            "--id",
            "cycle",
            "--category",
            "lesson",
            "--text",
            "cross category",
        ],
        &memory_home,
    );
    assert!(!wrong_category.status.success());
    assert!(String::from_utf8_lossy(&wrong_category.stderr).contains("MEMORY_CATEGORY_CHANGE"));

    let source_path = root.join("memory/knowledge.jsonl");
    let mut changed = fs::read_to_string(&source_path).unwrap();
    changed.push_str(r#"{"id":"drifted","category":"knowledge","content":"added outside index","tags":[],"source_refs":[]}"#);
    changed.push('\n');
    fs::write(&source_path, &changed).unwrap();
    let stale = run_memory(&root, &["verify"], &memory_home);
    assert!(stale.status.success());
    let stale: Value = serde_json::from_slice(&stale.stdout).unwrap();
    assert_eq!(stale["ok"], false);
    assert!(stale["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|scope| { scope["scope"] == "project" && scope["source_consistent"] == false }));
    let refreshed = run_memory(&root, &["query", "drifted"], &memory_home);
    assert!(refreshed.status.success());
    let refreshed: Value = serde_json::from_slice(&refreshed.stdout).unwrap();
    assert!(refreshed["keyword_matches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == "drifted"));
    let healthy = run_memory(&root, &["verify"], &memory_home);
    let healthy: Value = serde_json::from_slice(&healthy.stdout).unwrap();
    assert_eq!(healthy["ok"], true);

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn project_memory_ignores_empty_collab_state_dir() {
    let root = temp_root("project-memory-empty-collab-state");
    let memory_home = temp_root("project-memory-empty-collab-state-home");
    let xdg_home = temp_root("project-memory-empty-collab-state-xdg");
    let caller = temp_root("project-memory-empty-collab-state-caller");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&memory_home).unwrap();
    fs::create_dir_all(&caller).unwrap();
    let run_dir = xdg_home.join("collab/runs/empty-env-run");
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(
        run_dir.join("notes.jsonl"),
        r#"{"event_id":"e1","node_id":"n1","step_id":"s1","status":"working"}
"#,
    )
    .unwrap();

    let output = Command::new(memory_binary())
        .args(["reentry", "--run", "empty-env-run"])
        .current_dir(&caller)
        .env("PROJECT_MEMORY_HOME", &memory_home)
        .env("COLLAB_STATE_DIR", "")
        .env("XDG_STATE_HOME", &xdg_home)
        .env("HOME", &caller)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reentered: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reentered["run_id"], "empty-env-run");
    assert_eq!(reentered["status"], "blocked");
    assert_eq!(
        reentered["notes"].as_str().unwrap(),
        run_dir.join("notes.jsonl").to_str().unwrap()
    );
    assert!(!caller.join("runs").exists());
    assert!(!root.join("runs").exists());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
    fs::remove_dir_all(xdg_home).unwrap();
    fs::remove_dir_all(caller).unwrap();
}

#[test]
fn project_memory_detail_paths_are_injective_for_slash_ids() {
    let root = temp_root("project-memory-slash-id-details");
    let memory_home = temp_root("project-memory-slash-id-details-home");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&memory_home).unwrap();

    for (id, text) in [("a/b", "slash id"), ("a--b", "dash id")] {
        let created = run_memory(
            &root,
            &["entry", "--id", id, "--category", "lesson", "--text", text],
            &memory_home,
        );
        assert!(
            created.status.success(),
            "{}",
            String::from_utf8_lossy(&created.stderr)
        );
    }

    let slash = run_memory(&root, &["get", "a/b"], &memory_home);
    assert!(slash.status.success());
    let slash: Value = serde_json::from_slice(&slash.stdout).unwrap();
    let slash_path = slash["matches"][0]["detail_path"].as_str().unwrap();
    assert!(slash_path.ends_with("L3/a%2Fb.md"), "{slash_path}");
    assert_eq!(
        fs::read_to_string(root.join(slash_path))
            .unwrap()
            .contains("slash id"),
        true
    );

    let dashes = run_memory(&root, &["get", "a--b"], &memory_home);
    assert!(dashes.status.success());
    let dashes: Value = serde_json::from_slice(&dashes.stdout).unwrap();
    let dashes_path = dashes["matches"][0]["detail_path"].as_str().unwrap();
    assert!(dashes_path.ends_with("L3/a--b.md"), "{dashes_path}");
    assert_ne!(slash_path, dashes_path);
    assert_eq!(
        fs::read_to_string(root.join(dashes_path))
            .unwrap()
            .contains("dash id"),
        true
    );

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn project_memory_imports_encoded_slash_id_detail() {
    let root = temp_root("project-memory-encoded-slash-detail");
    let memory_home = temp_root("project-memory-encoded-slash-detail-home");
    fs::create_dir_all(root.join("memory/L3")).unwrap();
    fs::create_dir_all(&memory_home).unwrap();
    let detail = "<!-- project-memory:v1 {\"id\":\"a/b\",\"category\":\"lesson\"} -->\n\n# Encoded\n\nEncoded slash id\n<!-- project-memory:end -->\n";
    fs::write(root.join("memory/L3/a--b.md"), detail).unwrap();

    let rejected = run_memory(&root, &["index"], &memory_home);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("MEMORY_DETAIL_FILENAME_MISMATCH"));

    fs::rename(
        root.join("memory/L3/a--b.md"),
        root.join("memory/L3/a%2Fb.md"),
    )
    .unwrap();
    let imported = run_memory(&root, &["index"], &memory_home);
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let found = run_memory(&root, &["get", "a/b"], &memory_home);
    assert!(found.status.success());
    let found: Value = serde_json::from_slice(&found.stdout).unwrap();
    assert_eq!(found["matches"][0]["content"], "Encoded slash id");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn project_memory_migration_and_reentry_are_resumable_and_source_preserving() {
    let root = temp_root("project-memory-migration");
    let memory_home = temp_root("project-memory-migration-home");
    let root_text = root.to_str().unwrap();
    fs::create_dir_all(root.join("memory")).unwrap();
    fs::create_dir_all(&memory_home).unwrap();
    let legacy = concat!(
        r#"{"id":"legacy-path","category":"path","content":"legacy node","tags":["node"],"source_refs":["old/path"]}"#,
        "\n",
        r#"{"id":"legacy-lesson","category":"lesson","text":"legacy lesson","tags":["old"]}"#,
        "\n"
    );
    fs::write(
        root.join("memory/path.jsonl"),
        r#"{"id":"legacy-path","category":"path","content":"legacy node","tags":["existing"]}
"#,
    )
    .unwrap();
    fs::write(root.join("memory/entries.jsonl"), legacy).unwrap();

    let migrated = run_memory(&root, &["migrate"], &memory_home);
    assert!(
        migrated.status.success(),
        "{}",
        String::from_utf8_lossy(&migrated.stderr)
    );
    let migrated: Value = serde_json::from_slice(&migrated.stdout).unwrap();
    assert_eq!(migrated["status"], "complete");
    assert_eq!(migrated["migrated_entries"], 2);
    assert_eq!(migrated["raw_sources_retained"], true);
    assert_eq!(
        fs::read_to_string(root.join("memory/entries.jsonl")).unwrap(),
        legacy
    );
    assert!(root.join("memory/path.jsonl").is_file());
    assert!(root.join("memory/lesson.jsonl").is_file());
    let migrated_path = run_memory(&root, &["get", "legacy-path"], &memory_home);
    let migrated_path: Value = serde_json::from_slice(&migrated_path.stdout).unwrap();
    let migrated_tags = migrated_path["matches"][0]["tags"].as_array().unwrap();
    assert!(migrated_tags.iter().any(|tag| tag == "existing"));
    assert!(migrated_tags.iter().any(|tag| tag == "node"));
    assert!(migrated_path["matches"][0]["source_refs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|source| source == "old/path"));
    let migrated_detail = root.join(migrated_path["matches"][0]["detail_path"].as_str().unwrap());
    assert!(migrated_detail.is_file());
    assert!(fs::read_to_string(root.join("memory/index.md"))
        .unwrap()
        .contains("### legacy-path"));
    let exported = run_memory(&root, &["export"], &memory_home);
    assert!(exported.status.success());
    let exported: Value = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(exported["project"]["entries"], 2);

    fs::create_dir_all(memory_home.join("collab/runs/reentry-run")).unwrap();
    fs::write(
        memory_home.join("collab/runs/reentry-run/notes.jsonl"),
        r#"{"event_id":"e1","node_id":"path-node","step_id":"step-2","status":"working"}
"#,
    )
    .unwrap();
    fs::remove_dir_all(memory_home.join("projects")).unwrap();
    let reentered = run_memory(&root, &["reentry", "--run", "reentry-run"], &memory_home);
    assert!(
        reentered.status.success(),
        "{}",
        String::from_utf8_lossy(&reentered.stderr)
    );
    let reentered: Value = serde_json::from_slice(&reentered.stdout).unwrap();
    assert_eq!(reentered["status"], "ready");
    assert_eq!(reentered["run_id"], "reentry-run");
    assert_eq!(reentered["preserves_run_id"], true);
    assert_eq!(reentered["index"]["rebuilt"], true);
    assert_eq!(reentered["resume_from"]["node_id"], "path-node");
    assert!(reentered["next_queries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|query| query == "path-node"));
    let explicit_project = run_memory(
        &memory_home,
        &["reentry", "--run", "reentry-run", root_text],
        &memory_home,
    );
    assert!(
        explicit_project.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit_project.stderr)
    );
    let explicit_project: Value = serde_json::from_slice(&explicit_project.stdout).unwrap();
    assert_eq!(explicit_project["run_id"], "reentry-run");
    assert_eq!(explicit_project["status"], "ready");

    let marker_path = root.join("memory/migration.json");
    let mut marker: Value =
        serde_json::from_str(&fs::read_to_string(&marker_path).unwrap()).unwrap();
    marker["status"] = Value::String("in_progress".into());
    fs::write(
        &marker_path,
        serde_json::to_string_pretty(&marker).unwrap() + "\n",
    )
    .unwrap();
    let resumed = run_memory(&root, &["migrate"], &memory_home);
    assert!(resumed.status.success());
    let resumed: Value = serde_json::from_slice(&resumed.stdout).unwrap();
    assert_eq!(resumed["status"], "complete");
    assert_eq!(resumed["resumed"], true);
    let already = run_memory(&root, &["migrate"], &memory_home);
    assert!(already.status.success());
    let already: Value = serde_json::from_slice(&already.stdout).unwrap();
    assert_eq!(already["status"], "already_complete");

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn project_memory_uses_review_levels_markdown_details_and_tag_search() {
    let root = temp_root("project-memory-review-levels");
    let memory_home = temp_root("project-memory-review-levels-home");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&memory_home).unwrap();

    let created = run_memory(
        &root,
        &[
            "entry",
            "--title",
            "Canonical memory title",
            "--text",
            "The full detail stays outside the short index",
            "--tag",
            "architecture",
            "--tag",
            "review",
        ],
        &memory_home,
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(created["memory_level"], 3);
    assert_eq!(created["review_status"], "unreviewed");
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["detail_path"], format!("memory/L3/{id}.md"));
    let detail = root.join(created["detail_path"].as_str().unwrap());
    assert!(detail.is_file());
    assert!(fs::read_to_string(&detail)
        .unwrap()
        .contains("The full detail stays outside the short index"));
    let index = fs::read_to_string(root.join("memory/index.md")).unwrap();
    assert!(index.contains("### Canonical memory title"));
    assert!(index.contains("- tags: `architecture`, `review`"));
    assert!(index.contains(&format!("details: [L3/{id}.md](L3/{id}.md)")));
    assert!(!index.contains("The full detail stays outside the short index"));
    assert!(index.contains("## Level 3"));
    assert!(index.contains(&format!(
        "- L3: Canonical memory title (knowledge) [architecture,review] -> L3/{id}.md"
    )));

    let tagged = run_memory(&root, &["query", "--tag", "architecture"], &memory_home);
    assert!(tagged.status.success());
    let tagged: Value = serde_json::from_slice(&tagged.stdout).unwrap();
    assert!(tagged["keyword_matches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == id && entry["tags"].as_array().unwrap().len() == 2));

    let promoted = run_memory(
        &root,
        &[
            "promote",
            "--id",
            id,
            "--level",
            "2",
            "--evidence",
            "review/run-1#memory-1",
        ],
        &memory_home,
    );
    assert!(
        promoted.status.success(),
        "{}",
        String::from_utf8_lossy(&promoted.stderr)
    );
    let promoted: Value = serde_json::from_slice(&promoted.stdout).unwrap();
    assert_eq!(promoted["level"], 2);
    let current = run_memory(&root, &["get", id], &memory_home);
    let current: Value = serde_json::from_slice(&current.stdout).unwrap();
    assert_eq!(current["matches"][0]["memory_level"], 2);
    assert_eq!(current["matches"][0]["review_status"], "reviewed");
    assert_eq!(
        current["matches"][0]["detail_path"],
        format!("memory/L2/{id}.md")
    );
    assert!(root.join(format!("memory/L2/{id}.md")).is_file());
    assert!(fs::read_to_string(root.join("memory/index.md"))
        .unwrap()
        .contains("## Level 2"));
    assert!(fs::read_to_string(root.join("memory/index.md"))
        .unwrap()
        .contains(&format!(
            "- L2: Canonical memory title (knowledge) [architecture,review] -> L2/{id}.md"
        )));

    fs::remove_dir_all(memory_home.join("projects")).unwrap();
    let rebuilt = run_memory(&root, &["query", "--tag", "review"], &memory_home);
    assert!(rebuilt.status.success());
    let rebuilt: Value = serde_json::from_slice(&rebuilt.stdout).unwrap();
    assert!(rebuilt["keyword_matches"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["id"] == id && entry["memory_level"] == 2));

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn project_memory_handwritten_l3_is_automatically_indexed_once() {
    for trigger in ["get", "index", "verify"] {
        let root = temp_root(&format!("memory-handwritten-{trigger}"));
        let memory_home = temp_root(&format!("memory-handwritten-home-{trigger}"));
        fs::create_dir_all(root.join("memory/L3")).unwrap();
        fs::create_dir_all(&memory_home).unwrap();
        // Exercise both an existing SQLite projection and a missing one.
        if trigger == "get" {
            assert!(run_memory(&root, &["index"], &memory_home).status.success());
        }
        for id in ["manual-a", "manual-b"] {
            fs::write(root.join(format!("memory/L3/{id}.md")), format!(
                "<!-- project-memory:v1 {{\"id\":\"{id}\",\"category\":\"knowledge\",\"tags\":[\"handwritten\"],\"memory_level\":1,\"review_status\":\"reviewed\",\"review_evidence\":[\"forged\"]}} -->\n\n# Manual title\n\nHandwritten fact\n<!-- project-memory:end -->\n"
            )).unwrap();
        }
        let args = if trigger == "get" {
            vec!["get", "manual-a"]
        } else {
            vec![trigger]
        };
        let result = run_memory(&root, &args, &memory_home);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        for id in ["manual-a", "manual-b"] {
            let result = run_memory(&root, &["get", id], &memory_home);
            let value: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(value["matches"][0]["content"], "Handwritten fact");
            assert_eq!(value["matches"][0]["memory_level"], 3);
            assert_eq!(value["matches"][0]["review_status"], "unreviewed");
            assert!(value["matches"][0]["review_evidence"]
                .as_array()
                .is_none_or(|v| v.is_empty()));
        }
        assert!(run_memory(&root, &["index"], &memory_home).status.success());
        assert_eq!(
            fs::read_to_string(root.join("memory/knowledge.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        let query = run_memory(&root, &["query", "--tag", "handwritten"], &memory_home);
        assert!(query.status.success());
        assert!(String::from_utf8_lossy(&query.stdout).contains("manual-b"));
    }
}

#[test]
fn project_memory_handwritten_l3_validates_batch_before_writes() {
    let root = temp_root("memory-handwritten-invalid");
    let memory_home = temp_root("memory-handwritten-invalid-home");
    fs::create_dir_all(root.join("memory/L3")).unwrap();
    fs::create_dir_all(&memory_home).unwrap();
    let valid = "<!-- project-memory:v1 {\"id\":\"a-valid\"} -->\n\n# Valid\n\nFact\n<!-- project-memory:end -->\n";
    fs::write(root.join("memory/L3/a-valid.md"), valid).unwrap();
    let invalid = "# Not a marked memory\n\nNo ID\n";
    fs::write(root.join("memory/L3/z-invalid.md"), invalid).unwrap();
    let result = run_memory(&root, &["index"], &memory_home);
    assert!(!result.status.success());
    assert!(!root.join("memory/knowledge.jsonl").exists());
    assert_eq!(
        fs::read_to_string(root.join("memory/L3/a-valid.md")).unwrap(),
        valid
    );
    assert_eq!(
        fs::read_to_string(root.join("memory/L3/z-invalid.md")).unwrap(),
        invalid
    );
    fs::remove_file(root.join("memory/L3/z-invalid.md")).unwrap();
    fs::rename(
        root.join("memory/L3/a-valid.md"),
        root.join("memory/L3/wrong-name.md"),
    )
    .unwrap();
    let result = run_memory(&root, &["index"], &memory_home);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("MEMORY_DETAIL_FILENAME_MISMATCH"));
    assert!(!root.join("memory/knowledge.jsonl").exists());
}

#[test]
fn project_memory_markdown_round_trip_import_is_idempotent() {
    let root = temp_root("project-memory-markdown-round-trip");
    let memory_home = temp_root("project-memory-markdown-round-trip-home");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&memory_home).unwrap();

    let created = run_memory(
        &root,
        &[
            "entry",
            "--id",
            "round-trip",
            "--category",
            "lesson",
            "--title",
            "Original title",
            "--text",
            "Original detail",
            "--tag",
            "compatibility",
        ],
        &memory_home,
    );
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(created["write_mode"], "one_shot");
    let detail = root.join(created["detail_path"].as_str().unwrap());
    let mut markdown = fs::read_to_string(&detail).unwrap();
    assert!(markdown.starts_with("<!-- project-memory:v1 "));
    markdown = markdown
        .replace("# Original title", "# Edited title")
        .replace("Original detail", "Edited detail");
    fs::write(&detail, markdown).unwrap();

    let imported = run_memory(&root, &["import"], &memory_home);
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let imported: Value = serde_json::from_slice(&imported.stdout).unwrap();
    assert_eq!(imported["status"], "complete");
    assert_eq!(imported["imported_entries"], 1);

    let current = run_memory(&root, &["get", "round-trip"], &memory_home);
    let current: Value = serde_json::from_slice(&current.stdout).unwrap();
    assert_eq!(current["matches"][0]["title"], "Edited title");
    assert_eq!(current["matches"][0]["content"], "Edited detail");
    assert_eq!(
        fs::read_to_string(root.join("memory/lesson.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2,
        "import appends an event and preserves the original event"
    );

    let repeated = run_memory(&root, &["import"], &memory_home);
    assert!(repeated.status.success());
    let repeated: Value = serde_json::from_slice(&repeated.stdout).unwrap();
    assert_eq!(repeated["status"], "already_current");
    assert_eq!(repeated["skipped_existing"], 1);
    assert_eq!(
        fs::read_to_string(root.join("memory/lesson.jsonl"))
            .unwrap()
            .lines()
            .count(),
        2,
        "repeating import must not append another event"
    );

    fs::create_dir_all(root.join("memory/details")).unwrap();
    let legacy_detail = root.join("memory/details/legacy-markdown.md");
    fs::write(
        &legacy_detail,
        "# Legacy Markdown\n\nLegacy detail\n\n---\n\n- id: `legacy-markdown`\n- tags: `legacy` `compatible`\n- source_refs: `old/export`\n",
    )
    .unwrap();
    let legacy_import = run_memory(&root, &["import"], &memory_home);
    assert!(legacy_import.status.success());
    let legacy_import: Value = serde_json::from_slice(&legacy_import.stdout).unwrap();
    assert_eq!(legacy_import["imported_entries"], 1);
    let legacy = run_memory(&root, &["get", "legacy-markdown"], &memory_home);
    let legacy: Value = serde_json::from_slice(&legacy.stdout).unwrap();
    assert_eq!(legacy["matches"][0]["content"], "Legacy detail");
    assert_eq!(legacy["matches"][0]["category"], "knowledge");
    assert!(legacy["matches"][0]["tags"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tag| tag == "compatible"));

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(memory_home).unwrap();
}

#[test]
fn optional_memory_does_not_block_governance_initialization() {
    let root = temp_root("optional-memory-init");
    assert!(run(&["new", root.to_str().unwrap()]).status.success());
    fs::remove_dir_all(root.join("memory")).unwrap();
    fs::write(root.join("memory"), "business file").unwrap();
    let result = run(&["init", root.to_str().unwrap()]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(root.join(".appsdk/project.json").is_file());
    assert!(String::from_utf8_lossy(&result.stderr).contains("MEMORY_DIR_INVALID"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_detects_declared_rule_source_drift_and_symlink() {
    let root = temp_root("guidance-rule-source-drift");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());

    let agents_target = root.join("project-agents.md");
    fs::write(&agents_target, "# Project rules\n").unwrap();
    fs::remove_file(root.join("AGENTS.md")).unwrap();
    symlink(&agents_target, root.join("AGENTS.md")).unwrap();
    let linked = run(&["guide", "compile", root_text]);
    assert!(!linked.status.success());
    assert!(String::from_utf8_lossy(&linked.stderr).contains("GUIDANCE_RULE_SOURCE_SYMLINK"));
    fs::remove_file(root.join("AGENTS.md")).unwrap();
    fs::write(root.join("AGENTS.md"), "# Project rules\n").unwrap();
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let outside = temp_root("guidance-control-symlink-target");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(root.join(".appsdk-control/guidance")).unwrap();
    symlink(
        &outside,
        root.join(".appsdk-control/guidance/task-control-symlink"),
    )
    .unwrap();
    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "mode": "debug",
            "goal_id": "goal-change-me",
            "task_id": "task-control-symlink",
            "module_id": "app-core",
            "objective": "reject redirected control state",
            "scope_paths": ["playground/experiments/**"],
            "steps": [{"step_id":"debug-1","node_id":"orient","action":"bind project context","owner":"app-core","expected_evidence":["orientation-record","function-map-binding","verification-map-binding"]}]
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    let redirected = run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-control-symlink",
        "--input",
        "plan.json",
    ]);
    assert!(!redirected.status.success());
    assert!(String::from_utf8_lossy(&redirected.stderr).contains("GUIDANCE_TASK_CONTROL_SYMLINK"));
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    fs::remove_file(root.join(".appsdk-control/guidance/task-control-symlink")).unwrap();
    fs::remove_dir_all(outside).unwrap();

    fs::write(
        root.join("plan.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "mode": "debug",
            "goal_id": "goal-change-me",
            "task_id": "task-rule-drift",
            "module_id": "app-core",
            "objective": "detect declared rule drift",
            "scope_paths": ["playground/experiments/**"],
            "steps": [{"step_id":"debug-1","node_id":"orient","action":"bind project context","owner":"app-core","expected_evidence":["orientation-record","function-map-binding","verification-map-binding"]}]
        }))
        .unwrap()
            + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-rule-drift",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    fs::write(root.join("AGENTS.md"), "# Project rules\n\nChanged.\n").unwrap();
    let status = run(&["guide", "next", root_text, "--task", "task-rule-drift"]);
    assert!(status.status.success());
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        status_json["reason_code"],
        "GUIDANCE_COMPILED_CONTEXT_DRIFT:rule_sources"
    );
    assert_eq!(status_json["next"]["command"], "appsdk guide compile");

    assert!(run(&["guide", "compile", root_text]).status.success());
    let revised_status = run(&["guide", "next", root_text, "--task", "task-rule-drift"]);
    assert!(revised_status.status.success());
    let revised_json: Value = serde_json::from_slice(&revised_status.stdout).unwrap();
    assert_eq!(
        revised_json["reason_code"],
        "GUIDANCE_CONTEXT_DRIFT:guidance_manifest"
    );
    assert_eq!(
        revised_json["next"]["revision_reason"],
        "guidance_manifest_changed"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_plan_rejects_concurrent_writer_lock() {
    let root = temp_root("guidance-task-lock");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);
    let control_dir = root.join(".appsdk-control/guidance/task-locked");
    fs::create_dir_all(&control_dir).unwrap();
    fs::write(
        control_dir.join("write.lock"),
        format!("pid={} op=plan created=0\n", std::process::id()),
    )
    .unwrap();

    fs::write(root.join("plan.json"), "{}").unwrap();
    let res = run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-locked",
        "--input",
        "plan.json",
    ]);
    assert!(!res.status.success());
    assert!(String::from_utf8_lossy(&res.stderr).contains("GUIDANCE_TASK_LOCKED"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn guidance_event_ledger_reports_bad_line_number() {
    let root = temp_root("guidance-events-line");
    let root_text = root.to_str().unwrap();
    assert!(run(&["new", root_text]).status.success());
    assert!(run(&["guide", "compile", root_text]).status.success());
    init_git(&root);

    let plan_file = root.join("plan.json");
    let proposal = serde_json::json!({
        "schema_version": 1,
        "mode": "develop",
        "goal_id": "goal-change-me",
        "task_id": "task-events",
        "module_id": "app-core",
        "objective": "line number for corrupted ledger",
        "scope_paths": ["playground/experiments/input.txt"],
        "steps": [{
            "step_id": "step-1",
            "node_id": "requirements",
            "action": "analyze requirements",
            "owner": "app-core",
            "expected_evidence": ["requirements"]
        }]
    });
    fs::create_dir_all(root.join("playground/experiments")).unwrap();
    fs::write(root.join("playground/experiments/input.txt"), "v1\n").unwrap();
    fs::write(
        &plan_file,
        serde_json::to_string_pretty(&proposal).unwrap() + "\n",
    )
    .unwrap();
    assert!(run(&[
        "guide",
        "plan",
        root_text,
        "--task",
        "task-events",
        "--input",
        "plan.json",
    ])
    .status
    .success());

    let events = root.join(".appsdk-control/guidance/task-events/events.jsonl");
    let mut content = fs::read_to_string(&events).unwrap();
    content.push_str("not-json\n");
    fs::write(&events, content).unwrap();

    let status = run(&["guide", "next", root_text, "--task", "task-events"]);
    assert!(!status.status.success());
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(
        stderr.contains("GUIDANCE_EVENTS_INVALID:line=2"),
        "{}",
        stderr
    );

    fs::remove_dir_all(root).unwrap();
}
