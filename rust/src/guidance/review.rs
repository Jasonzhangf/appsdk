//! Human tour and review state for a generated Guide workflow.
//!
//! This is deliberately a projection over the existing append-only guidance
//! event ledger.  It does not edit the compiled manifest or the active plan:
//! accepted node and flow revisions are staged records that a later, explicit
//! source update can publish.

use super::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

fn plan_mode(plan: Option<&Value>, mode: Option<&str>) -> String {
    mode.map(ToOwned::to_owned)
        .or_else(|| {
            plan.and_then(|plan| plan.get("mode").and_then(Value::as_str))
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| {
            fail(
                "GUIDANCE_TOUR_MODE_REQUIRED",
                "provide --mode or create a plan first",
            )
        })
}

fn workflow_path(workflow: &Value, plan: Option<&Value>) -> Vec<String> {
    if let Some(plan) = plan {
        let steps = required_array(plan, "steps", "GUIDANCE_PLAN_INVALID");
        let path = steps
            .iter()
            .map(|step| required_string(step, "node_id", "GUIDANCE_PLAN_STEP_INVALID").to_string())
            .collect::<Vec<_>>();
        if !path.is_empty() {
            return path;
        }
    }
    vec![required_string(workflow, "entry_node", "GUIDANCE_WORKFLOW_INVALID").to_string()]
}

fn workflow_nodes(workflow: &Value) -> &Vec<Value> {
    required_array(workflow, "nodes", "GUIDANCE_WORKFLOW_INVALID")
}

fn node<'a>(workflow: &'a Value, node_id: &str) -> &'a Value {
    workflow_nodes(workflow)
        .iter()
        .find(|node| node.get("node_id").and_then(Value::as_str) == Some(node_id))
        .unwrap_or_else(|| {
            fail(
                format!("GUIDANCE_UNKNOWN_NODE:{}", node_id),
                "select a declared workflow node",
            )
        })
}

fn assert_path(workflow: &Value, path: &[String]) {
    if path.is_empty() {
        fail(
            "GUIDANCE_TOUR_PATH_EMPTY",
            "select at least one workflow node",
        );
    }
    let entry = required_string(workflow, "entry_node", "GUIDANCE_WORKFLOW_INVALID");
    if path.first().map(String::as_str) != Some(entry) {
        fail(
            format!("GUIDANCE_TOUR_ENTRY_MISMATCH:{}", entry),
            "start the tour at the declared workflow entry node",
        );
    }
    let edges = required_array(workflow, "edges", "GUIDANCE_WORKFLOW_INVALID");
    for pair in path.windows(2) {
        if !edges.iter().any(|edge| {
            edge.get("from").and_then(Value::as_str) == Some(pair[0].as_str())
                && edge.get("to").and_then(Value::as_str) == Some(pair[1].as_str())
        }) {
            fail(
                format!("GUIDANCE_TOUR_NON_ADJACENT:{}:{}", pair[0], pair[1]),
                "choose only declared adjacent workflow edges",
            );
        }
    }
    let mut seen = HashSet::new();
    for id in path {
        node(workflow, id);
        if !seen.insert(id) {
            fail(
                format!("GUIDANCE_TOUR_DUPLICATE_NODE:{}", id),
                "choose each workflow node at most once in a tour path",
            );
        }
    }
}

fn latest_tour<'a>(events: &'a [Value], task: &str) -> Option<&'a Value> {
    events.iter().rev().find(|event| {
        event.get("record_type").and_then(Value::as_str) == Some("GuideTourRecord")
            && event.get("task_id").and_then(Value::as_str) == Some(task)
    })
}

fn tour_path(events: &[Value], task: &str) -> Option<Vec<String>> {
    latest_tour(events, task).and_then(|tour| {
        tour.get("selected_path")
            .and_then(Value::as_array)
            .map(|path| {
                path.iter()
                    .map(|value| {
                        value
                            .as_str()
                            .unwrap_or_else(|| {
                                fail("GUIDANCE_TOUR_RECORD_INVALID", "repair the tour event")
                            })
                            .to_string()
                    })
                    .collect()
            })
    })
}

fn review_stage(value: &str) -> &'static str {
    match value {
        "node" | "nodes" | "node_review" | "node_content" | "node_content_review" => "node_review",
        "flow" | "flows" | "flow_review" | "process" | "process_review" => "flow_review",
        _ => fail(
            format!("GUIDANCE_REVIEW_STAGE_INVALID:{}", value),
            "use node_review or flow_review",
        ),
    }
}

fn review_verdict(value: &str) -> &'static str {
    match value {
        "accept" | "accepted" | "approve" | "approved" | "pass" => "accepted",
        "reject" | "rejected" | "deny" | "denied" | "fail" => "rejected",
        "pending" => "pending",
        _ => fail(
            format!("GUIDANCE_REVIEW_VERDICT_INVALID:{}", value),
            "use accept, reject, or pending",
        ),
    }
}

fn input_updates<'a>(input: &'a Value, key: &str) -> &'a Vec<Value> {
    input
        .get(key)
        .or_else(|| input.get("updates"))
        .or_else(|| input.get("nodes"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| fail("GUIDANCE_REVIEW_UPDATES_REQUIRED", "declare review updates"))
}

fn accepted_nodes<'a>(events: &'a [Value], task: &str) -> HashMap<String, (String, &'a Value)> {
    let mut accepted = HashMap::new();
    for event in events.iter().filter(|event| {
        event.get("record_type").and_then(Value::as_str) == Some("GuideReviewRecord")
            && event.get("task_id").and_then(Value::as_str) == Some(task)
            && event.get("stage").and_then(Value::as_str) == Some("node_review")
    }) {
        let Some(updates) = event.get("node_updates").and_then(Value::as_array) else {
            continue;
        };
        for update in updates {
            let Some(node_id) = update.get("node_id").and_then(Value::as_str) else {
                continue;
            };
            if update.get("verdict").and_then(Value::as_str) == Some("accepted") {
                let Some(revision_id) = update.get("revision_id").and_then(Value::as_str) else {
                    continue;
                };
                accepted.insert(node_id.to_string(), (revision_id.to_string(), event));
            } else {
                accepted.remove(node_id);
            }
        }
    }
    accepted
}

fn render_tour(
    workflow: &Value,
    path: &[String],
    events: &[Value],
    task: &str,
    mode: &str,
) -> Value {
    let accepted = accepted_nodes(events, task);
    let nodes = path
        .iter()
        .map(|node_id| {
            let source = node(workflow, node_id);
            let (status, revision_id) = accepted
                .get(node_id)
                .map(|(revision, _)| ("accepted", Some(revision.as_str())))
                .unwrap_or(("pending", None));
            let mut value = serde_json::json!({
                "node_id": node_id,
                "instruction": source["instruction"],
                "evidence": source["evidence"],
                "review_status": status
            });
            if let Some(revision_id) = revision_id {
                value["revision_id"] = Value::String(revision_id.to_string());
            }
            value
        })
        .collect::<Vec<_>>();
    let all_nodes_accepted = nodes
        .iter()
        .all(|node| node.get("review_status").and_then(Value::as_str) == Some("accepted"));
    serde_json::json!({
        "harness": "appsdk-development-process-control",
        "task_id": task,
        "mode": mode,
        "tour": {"selected_path": path, "nodes": nodes},
        "stage": if all_nodes_accepted { "flow_review" } else { "node_review" },
        "node_review_complete": all_nodes_accepted,
        "flow_review_allowed": all_nodes_accepted,
        "next": if all_nodes_accepted { "submit flow_review" } else { "review each selected node's content and accept it" },
        "writes_state": false
    })
}

pub(super) fn tour(root: &Path, task: &str, mode: Option<&str>, input: Option<&str>) {
    assert_owner_worktree(root);
    let project = read_project(root);
    let compiled = load_compiled(root, &project)
        .unwrap_or_else(|| fail("GUIDANCE_NOT_COMPILED", "run appsdk guide compile"));
    let existing_plan = plan_file(root, task)
        .is_file()
        .then(|| read_plan(root, task));
    let mode = plan_mode(existing_plan.as_ref(), mode);
    let selected_workflow = workflow(&compiled, &mode);
    let default_path = workflow_path(selected_workflow, existing_plan.as_ref());
    let events = read_events(root, task);
    let selected_path = tour_path(&events, task).unwrap_or_else(|| default_path.clone());
    assert_path(selected_workflow, &selected_path);
    let Some(input) = input else {
        output(&render_tour(
            selected_workflow,
            &selected_path,
            &events,
            task,
            &mode,
        ));
        return;
    };
    let input_relative = safe_relative(input, "GUIDANCE_INPUT_PATH_ESCAPE");
    assert_no_symlink(root, &input_relative, "GUIDANCE_INPUT_SYMLINK");
    let value = read_json(&root.join(input_relative), "GUIDANCE_TOUR_INPUT_INVALID");
    if value.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail(
            "GUIDANCE_TOUR_SCHEMA_INVALID",
            "submit tour schema version 1",
        );
    }
    let path = value
        .get("selected_path")
        .or_else(|| value.get("path"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value.as_str().unwrap_or_else(|| {
                        fail(
                            "GUIDANCE_TOUR_PATH_INVALID",
                            "use node IDs in selected_path",
                        )
                    })
                })
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or(default_path);
    assert_path(selected_workflow, &path);
    let tour_id = value
        .get("tour_id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("GUIDANCE_TOUR_ID_REQUIRED", "declare a stable tour_id"));
    assert_id(tour_id, "GUIDANCE_TOUR_ID_INVALID");
    if let Some(previous) = events.iter().rev().find(|event| {
        event.get("record_type").and_then(Value::as_str) == Some("GuideTourRecord")
            && event.get("task_id").and_then(Value::as_str) == Some(task)
            && event.get("tour_id").and_then(Value::as_str) == Some(tour_id)
    }) {
        let input_hash = digest_value(&value);
        if previous.get("input_hash").and_then(Value::as_str) == Some(input_hash.as_str()) {
            output(&serde_json::json!({"accepted": true, "idempotent": true, "tour_id": tour_id}));
            return;
        }
        fail(
            format!("GUIDANCE_TOUR_CONFLICT:{}", tour_id),
            "use a new tour_id for different content",
        );
    }
    let event = serde_json::json!({
        "schema_version": 1,
        "record_type": "GuideTourRecord",
        "tour_id": tour_id,
        "task_id": task,
        "mode": mode,
        "selected_path": path,
        "input_hash": digest_value(&value),
        "created_at": Utc::now().to_rfc3339()
    });
    append_event(&event_file(root, task), &event);
    output(
        &serde_json::json!({"accepted": true, "idempotent": false, "tour_id": tour_id, "stage": "node_review", "selected_path": event["selected_path"]}),
    );
}

fn node_review(
    root: &Path,
    task: &str,
    workflow: &Value,
    path: &[String],
    input: &Value,
    events: &[Value],
) -> Value {
    let updates = input_updates(input, "node_updates");
    let path_set = path.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut normalized = Vec::new();
    for update in updates {
        let node_id = required_string(update, "node_id", "GUIDANCE_NODE_REVIEW_INVALID");
        if !path_set.contains(&node_id) {
            fail(
                format!("GUIDANCE_NODE_NOT_IN_TOUR:{}", node_id),
                "review only nodes in the selected tour path",
            );
        }
        let source = node(workflow, node_id);
        let verdict = review_verdict(required_string(
            update,
            "verdict",
            "GUIDANCE_NODE_REVIEW_INVALID",
        ));
        let content = update
            .get("content")
            .or_else(|| update.get("node_content"))
            .or_else(|| update.get("patch"))
            .cloned()
            .unwrap_or_else(|| source["instruction"].clone());
        let revision_id = format!("node-{}-{}", node_id, &digest_value(&content)[7..19]);
        normalized.push(serde_json::json!({
            "node_id": node_id,
            "verdict": verdict,
            "content": content,
            "revision_id": revision_id
        }));
    }
    let review_id = input
        .get("review_id")
        .or_else(|| input.get("event_id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("GUIDANCE_REVIEW_ID_REQUIRED", "declare a stable review_id"));
    assert_id(review_id, "GUIDANCE_REVIEW_ID_INVALID");
    let all_accepted = {
        let mut state = accepted_nodes(events, task)
            .into_iter()
            .map(|(id, (revision, _))| (id, revision))
            .collect::<HashMap<_, _>>();
        for update in &normalized {
            if update["verdict"] == "accepted" {
                state.insert(
                    required_string(update, "node_id", "GUIDANCE_NODE_REVIEW_INVALID").to_string(),
                    required_string(update, "revision_id", "GUIDANCE_NODE_REVIEW_INVALID")
                        .to_string(),
                );
            } else {
                state.remove(required_string(
                    update,
                    "node_id",
                    "GUIDANCE_NODE_REVIEW_INVALID",
                ));
            }
        }
        path.iter().all(|id| state.contains_key(id))
    };
    let verdict = if normalized
        .iter()
        .any(|update| update["verdict"] == "rejected")
    {
        "rejected"
    } else if all_accepted {
        "accepted"
    } else {
        "pending"
    };
    let event = serde_json::json!({
        "schema_version": 1,
        "record_type": "GuideReviewRecord",
        "review_id": review_id,
        "task_id": task,
        "stage": "node_review",
        "verdict": verdict,
        "node_updates": normalized,
        "input_hash": digest_value(input),
        "created_at": Utc::now().to_rfc3339()
    });
    append_event(&event_file(root, task), &event);
    serde_json::json!({
        "accepted": true,
        "review_id": review_id,
        "stage": "node_review",
        "verdict": verdict,
        "node_review_complete": all_accepted,
        "flow_review_allowed": all_accepted,
        "next": if all_accepted { "submit flow_review" } else { "review and accept the remaining node content" }
    })
}

fn flow_review(
    root: &Path,
    task: &str,
    _workflow: &Value,
    path: &[String],
    input: &Value,
    events: &[Value],
) -> Value {
    let accepted = accepted_nodes(events, task);
    if path.iter().any(|node_id| !accepted.contains_key(node_id)) {
        fail(
            "GUIDANCE_FLOW_REVIEW_REQUIRES_NODE_APPROVAL",
            "review and accept every selected node before changing flow edges, order, or rules",
        );
    }
    let patch = input
        .get("flow_update")
        .or_else(|| input.get("flow"))
        .unwrap_or_else(|| {
            fail(
                "GUIDANCE_FLOW_UPDATE_REQUIRED",
                "declare flow_update with edges, order, sequence, or rules",
            )
        });
    if !["edges", "order", "sequence", "rules"]
        .iter()
        .any(|key| patch.get(*key).is_some())
    {
        fail(
            "GUIDANCE_FLOW_UPDATE_EMPTY",
            "declare edges, order, sequence, or rules in flow_update",
        );
    }
    let edges = patch
        .get("edges")
        .map(|value| {
            value.as_array().unwrap_or_else(|| {
                fail(
                    "GUIDANCE_FLOW_UPDATE_INVALID",
                    "declare flow edges as an array",
                )
            })
        })
        .map_or(&[][..], |v| v.as_slice());
    let path_set = path.iter().map(String::as_str).collect::<HashSet<_>>();
    for edge in edges {
        let from = required_string(edge, "from", "GUIDANCE_FLOW_UPDATE_INVALID");
        let to = required_string(edge, "to", "GUIDANCE_FLOW_UPDATE_INVALID");
        if from == to || !path_set.contains(&from) || !path_set.contains(&to) {
            fail(
                "GUIDANCE_FLOW_UPDATE_NODE_INVALID",
                "flow updates must reference approved tour nodes",
            );
        }
    }
    let order = patch.get("order").or_else(|| patch.get("sequence"));
    if let Some(order) = order {
        let order = order
            .as_array()
            .unwrap_or_else(|| fail("GUIDANCE_FLOW_ORDER_INVALID", "use node IDs in order"));
        let mut seen = HashSet::new();
        for value in order {
            let id = value
                .as_str()
                .unwrap_or_else(|| fail("GUIDANCE_FLOW_ORDER_INVALID", "use node IDs in order"));
            if !path_set.contains(&id) || !seen.insert(id) {
                fail(
                    "GUIDANCE_FLOW_ORDER_INVALID",
                    "flow order must use each approved tour node at most once",
                );
            }
        }
    }
    if let Some(rules) = patch.get("rules") {
        if !rules.is_array() {
            fail(
                "GUIDANCE_FLOW_RULES_INVALID",
                "declare flow rules as an array",
            );
        }
    }
    let review_id = input
        .get("review_id")
        .or_else(|| input.get("event_id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| fail("GUIDANCE_REVIEW_ID_REQUIRED", "declare a stable review_id"));
    assert_id(review_id, "GUIDANCE_REVIEW_ID_INVALID");
    let verdict = review_verdict(
        input
            .get("verdict")
            .and_then(Value::as_str)
            .unwrap_or("accept"),
    );
    let node_revision_ids = path
        .iter()
        .map(|id| {
            accepted
                .get(id)
                .map(|(revision, _)| revision.clone())
                .unwrap()
        })
        .collect::<Vec<_>>();
    let flow_revision = format!("flow-{}", &digest_value(patch)[7..19]);
    let event = serde_json::json!({
        "schema_version": 1,
        "record_type": "GuideReviewRecord",
        "review_id": review_id,
        "task_id": task,
        "stage": "flow_review",
        "verdict": verdict,
        "flow_revision": flow_revision,
        "node_revision_ids": node_revision_ids,
        "flow_patch": patch,
        "input_hash": digest_value(input),
        "created_at": Utc::now().to_rfc3339()
    });
    append_event(&event_file(root, task), &event);
    serde_json::json!({
        "accepted": true,
        "review_id": review_id,
        "stage": "flow_review",
        "verdict": verdict,
        "flow_revision": if verdict == "accepted" { event["flow_revision"].clone() } else { Value::Null },
        "node_revision_ids": event["node_revision_ids"],
        "active_revision_changed": false,
        "next": if verdict == "accepted" { "apply the reviewed flow_patch to the declared process source, then compile" } else { "revise flow edges, order, or rules and submit flow_review again" }
    })
}

pub(super) fn state(root: &Path, task: &str) -> Value {
    let events = read_events(root, task);
    let Some(plan) = plan_file(root, task)
        .is_file()
        .then(|| read_plan(root, task))
    else {
        return serde_json::json!({"stage":"tour","tour_started":false,"node_review_complete":false,"flow_review_allowed":false});
    };
    let mode = required_string(&plan, "mode", "GUIDANCE_PLAN_INVALID");
    let project = read_project(root);
    let Some(compiled) = load_compiled(root, &project) else {
        return serde_json::json!({"stage":"tour","tour_started":false,"node_review_complete":false,"flow_review_allowed":false});
    };
    let selected_workflow = workflow(&compiled, mode);
    let path =
        tour_path(&events, task).unwrap_or_else(|| workflow_path(selected_workflow, Some(&plan)));
    let accepted = accepted_nodes(&events, task);
    let node_review_complete = path.iter().all(|id| accepted.contains_key(id));
    let latest_flow_index = events.iter().rposition(|event| {
        event.get("record_type").and_then(Value::as_str) == Some("GuideReviewRecord")
            && event.get("task_id").and_then(Value::as_str) == Some(task)
            && event.get("stage").and_then(Value::as_str) == Some("flow_review")
    });
    let latest_node_index = events.iter().rposition(|event| {
        event.get("record_type").and_then(Value::as_str) == Some("GuideReviewRecord")
            && event.get("task_id").and_then(Value::as_str) == Some(task)
            && event.get("stage").and_then(Value::as_str) == Some("node_review")
    });
    let flow_accepted = latest_flow_index
        .filter(|flow_index| {
            latest_node_index
                .map(|node_index| *flow_index > node_index)
                .unwrap_or(true)
        })
        .is_some_and(|flow_index| {
            events[flow_index].get("verdict").and_then(Value::as_str) == Some("accepted")
        });
    serde_json::json!({
        "stage": if flow_accepted { "complete" } else if node_review_complete { "flow_review" } else { "node_review" },
        "tour_started": latest_tour(&events, task).is_some(),
        "selected_path": path,
        "node_review_complete": node_review_complete,
        "flow_review_allowed": node_review_complete,
        "flow_review_complete": flow_accepted,
        "active_revision_changed": false
    })
}

pub(super) fn review(root: &Path, task: &str, input: &str) {
    assert_owner_worktree(root);
    let project = read_project(root);
    let compiled = load_compiled(root, &project)
        .unwrap_or_else(|| fail("GUIDANCE_NOT_COMPILED", "run appsdk guide compile"));
    let plan = read_plan(root, task);
    let mode = required_string(&plan, "mode", "GUIDANCE_PLAN_INVALID");
    let selected_workflow = workflow(&compiled, mode);
    let events = read_events(root, task);
    let path =
        tour_path(&events, task).unwrap_or_else(|| workflow_path(selected_workflow, Some(&plan)));
    assert_path(selected_workflow, &path);
    let input_relative = safe_relative(input, "GUIDANCE_INPUT_PATH_ESCAPE");
    assert_no_symlink(root, &input_relative, "GUIDANCE_INPUT_SYMLINK");
    let value = read_json(&root.join(input_relative), "GUIDANCE_REVIEW_INPUT_INVALID");
    if value.get("schema_version").and_then(Value::as_u64) != Some(1) {
        fail(
            "GUIDANCE_REVIEW_SCHEMA_INVALID",
            "submit review schema version 1",
        );
    }
    let stage = review_stage(required_string(
        &value,
        "stage",
        "GUIDANCE_REVIEW_STAGE_REQUIRED",
    ));
    let review_id = required_string(&value, "review_id", "GUIDANCE_REVIEW_ID_REQUIRED");
    assert_id(review_id, "GUIDANCE_REVIEW_ID_INVALID");
    let input_hash = digest_value(&value);
    if let Some(previous) = events.iter().rev().find(|event| {
        event.get("record_type").and_then(Value::as_str) == Some("GuideReviewRecord")
            && event.get("task_id").and_then(Value::as_str) == Some(task)
            && event.get("review_id").and_then(Value::as_str) == Some(review_id)
    }) {
        if previous.get("input_hash").and_then(Value::as_str) == Some(input_hash.as_str()) {
            output(
                &serde_json::json!({"accepted": true, "idempotent": true, "review_id": review_id, "stage": previous["stage"], "verdict": previous["verdict"]}),
            );
            return;
        }
        fail(
            format!("GUIDANCE_REVIEW_CONFLICT:{}", review_id),
            "use a new review_id for different content",
        );
    }
    let result = match stage {
        "node_review" => node_review(root, task, selected_workflow, &path, &value, &events),
        "flow_review" => flow_review(root, task, selected_workflow, &path, &value, &events),
        _ => unreachable!(),
    };
    output(&result);
}
