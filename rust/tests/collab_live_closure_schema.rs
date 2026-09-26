use jsonschema::validator_for;
use serde_json::{json, Value};

const CLOSURE_SCHEMA_SOURCE: &str =
    include_str!("../../contracts/records/collab-live-closure-record.schema.json");
const PROMOTION_SCHEMA_SOURCE: &str =
    include_str!("../../contracts/records/promotion-record.schema.json");
const RECORD_GRAPH_SOURCE: &str =
    include_str!("../../contracts/records/record-graph.contract.json");
const LIFECYCLE_STATE_MACHINES_SOURCE: &str =
    include_str!("../../contracts/lifecycle-state-machines.manifest.json");
const FUNCTION_MAP_SOURCE: &str = include_str!("../../contracts/maps/function-map.json");
const MAINLINE_CALL_MAP_SOURCE: &str = include_str!("../../contracts/maps/mainline-call-map.json");

const PATHS: [&str; 7] = [
    "peer_to_peer",
    "peer_to_master",
    "master_to_peer",
    "master_to_master",
    "daemon_to_peer",
    "daemon_to_master",
    "restart_replay",
];

fn validator(source: &str) -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(source).expect("schema must parse");
    validator_for(&schema).expect("schema must be a valid Draft 2020-12 document")
}

fn receipt(path: &str) -> Value {
    let (sender, receiver) = match path {
        "peer_to_peer" => ("peer", "peer"),
        "peer_to_master" => ("peer", "master"),
        "master_to_peer" => ("master", "peer"),
        "master_to_master" => ("master", "master"),
        "daemon_to_peer" => ("daemon", "peer"),
        "daemon_to_master" => ("daemon", "master"),
        "restart_replay" => ("daemon", "peer"),
        _ => unreachable!(),
    };
    json!({
        "evidence_id": format!("evidence-{path}"),
        "message_id": format!("message-{path}"),
        "challenge": format!("challenge-{path}"),
        "sender": sender,
        "receiver": receiver,
        "source_commit": "source-1",
        "artifact_hash": "artifact-1",
        "environment_id": "environment-1",
        "entrypoint": "entrypoint-1",
        "endpoint_generation": 1,
        "observed_at": "2026-01-01T00:00:00Z"
    })
}

fn closure() -> Value {
    let mut evidence_ids = serde_json::Map::new();
    let mut path_receipts = serde_json::Map::new();
    for path in PATHS {
        evidence_ids.insert(path.to_string(), json!(format!("evidence-{path}")));
        path_receipts.insert(path.to_string(), receipt(path));
    }
    json!({
        "closure_id": "closure-1",
        "issue_id": "issue-1",
        "module_id": "module-1",
        "fix_candidate_id": "candidate-1",
        "artifact_hash": "artifact-1",
        "scope_hash": "scope-1",
        "source_commit": "source-1",
        "environment_id": "environment-1",
        "entrypoint": "entrypoint-1",
        "collab_identity": {
            "worker_id": "worker-1",
            "binding_id": "binding-1",
            "thread_id": "thread-1",
            "app_scope_id": "app-1",
            "project_scope_id": "project-1"
        },
        "route_receipt": {
            "daemon_live": true,
            "endpoint_generation": 1,
            "route_scope": {
                "project_scope": "project-1",
                "app_scope_id": "app-1"
            },
            "storage_root": "storage-1",
            "tmux_endpoint": {
                "socket_path": "/tmp/collab-live-closure.sock",
                "server_pid": 12345,
                "tmux_session_id": "$1",
                "pane_id": "%1",
                "pane_pid": 23456,
                "codex_session_id": "session-1",
                "codex_thread_id": "thread-1"
            },
            "resolved_at": "2026-01-01T00:00:00Z",
            "source": "collab_cli"
        },
        "evidence_ids": Value::Object(evidence_ids),
        "path_receipts": Value::Object(path_receipts),
        "created_at": "2026-01-01T00:00:00Z"
    })
}

fn promotion() -> Value {
    json!({
        "promotion_id": "promotion-1",
        "issue_id": "issue-1",
        "experiment_id": "experiment-1",
        "module_id": "module-1",
        "worktree_record_id": "worktree-1",
        "reproduction_record_id": "reproduction-1",
        "fix_candidate_id": "candidate-1",
        "architecture_review_id": "review-1",
        "effectiveness_record_id": "effectiveness-1",
        "merge_record_id": "merge-1",
        "base_commit": "base-1",
        "candidate_commit": "candidate-commit-1",
        "merged_commit": "merged-1",
        "source_commit": "source-1",
        "previous_active_version": null,
        "new_active_version": "0.1.0007",
        "review_id": "review-1",
        "evidence_ids": ["evidence-1"],
        "required_gate_results": [
            {"gate_id": "goal_confirmed", "result": "pass", "producer": "test"}
        ],
        "change_set_id": "change-1",
        "compatibility_level": "compatible",
        "root_cause": "root cause",
        "design_id": "design-1",
        "change_reason_comment": "reason",
        "playground_cleanup_record_id": "cleanup-1",
        "bug_closure_verified": true,
        "created_at": "2026-01-01T00:00:00Z"
    })
}

#[test]
fn schema_accepts_distinct_path_bound_receipts() {
    assert!(validator(CLOSURE_SCHEMA_SOURCE).is_valid(&closure()));
}

#[test]
fn schema_rejects_direction_mismatch_for_every_path() {
    let validator = validator(CLOSURE_SCHEMA_SOURCE);
    for path in PATHS {
        let mut wrong_sender = closure();
        let current_sender = wrong_sender["path_receipts"][path]["sender"]
            .as_str()
            .unwrap();
        let replacement_sender = if current_sender == "peer" {
            "master"
        } else {
            "peer"
        };
        wrong_sender["path_receipts"][path]["sender"] = json!(replacement_sender);
        assert!(
            !validator.is_valid(&wrong_sender),
            "schema accepted wrong sender for {path}"
        );

        let mut wrong_receiver = closure();
        let current_receiver = wrong_receiver["path_receipts"][path]["receiver"]
            .as_str()
            .unwrap();
        let replacement_receiver = if current_receiver == "peer" {
            "master"
        } else {
            "peer"
        };
        wrong_receiver["path_receipts"][path]["receiver"] = json!(replacement_receiver);
        assert!(
            !validator.is_valid(&wrong_receiver),
            "schema accepted wrong receiver for {path}"
        );
    }
}

#[test]
fn schema_rejects_unknown_receipt_fields_and_empty_identities() {
    let validator = validator(CLOSURE_SCHEMA_SOURCE);
    let mut unknown_field = closure();
    unknown_field["path_receipts"]["daemon_to_master"]["unexpected"] = json!(true);
    assert!(!validator.is_valid(&unknown_field));

    let mut unknown_path = closure();
    unknown_path["path_receipts"]["unknown"] = receipt("peer_to_peer");
    assert!(!validator.is_valid(&unknown_path));

    let mut empty_evidence = closure();
    empty_evidence["evidence_ids"]["restart_replay"] = json!("");
    assert!(!validator.is_valid(&empty_evidence));

    let mut empty_message = closure();
    empty_message["path_receipts"]["restart_replay"]["message_id"] = json!("");
    assert!(!validator.is_valid(&empty_message));
}

#[test]
fn promotion_schema_requires_live_closure_for_collaboration_promotions() {
    let validator = validator(PROMOTION_SCHEMA_SOURCE);
    let single_worker = promotion();
    assert!(validator.is_valid(&single_worker));

    let mut missing_closure = promotion();
    missing_closure["collaboration_record_id"] = json!("collaboration-1");
    assert!(!validator.is_valid(&missing_closure));

    let mut bound_closure = missing_closure;
    bound_closure["collab_live_closure_record_id"] = json!("closure-1");
    assert!(validator.is_valid(&bound_closure));
}

#[test]
fn lifecycle_graph_and_producer_place_live_closure_before_promotion() {
    let record_graph: Value = serde_json::from_str(RECORD_GRAPH_SOURCE).unwrap();
    let parallel_order = record_graph["properties"]["fix_lifecycle"]["properties"]
        ["parallel_order"]["const"]
        .as_array()
        .unwrap();
    let closure_index = parallel_order
        .iter()
        .position(|value| value == "collab_live_closed")
        .expect("parallel order must include collab_live_closed");
    let promotion_index = parallel_order
        .iter()
        .position(|value| value == "promotion_recorded")
        .expect("parallel order must include promotion_recorded");
    assert!(closure_index < promotion_index);

    let state_machines: Value = serde_json::from_str(LIFECYCLE_STATE_MACHINES_SOURCE).unwrap();
    let transitions = state_machines["issue"]["transitions"].as_array().unwrap();
    let closure_transition = transitions
        .iter()
        .find(|transition| {
            transition["from"] == "remote_verified" && transition["to"] == "collab_live_closed"
        })
        .expect("remote verification must transition through live closure");
    assert!(closure_transition["requirements"]
        .as_array()
        .unwrap()
        .iter()
        .any(|requirement| requirement == "collab_live_closure"));
    assert!(transitions.iter().any(|transition| {
        transition["from"] == "collab_live_closed" && transition["to"] == "promoted"
    }));

    let function_map: Value = serde_json::from_str(FUNCTION_MAP_SOURCE).unwrap();
    let producer = function_map["functions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|function| function["function_id"] == "lifecycle_chain_record_producer")
        .expect("lifecycle chain producer must be declared");
    assert!(producer["required_gates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|gate| gate == "collab_live_closure"));
}

#[test]
fn mainline_call_map_names_the_live_closure_owner_and_record_writer() {
    let map: Value = serde_json::from_str(MAINLINE_CALL_MAP_SOURCE).unwrap();
    let edges = map["edges"].as_array().unwrap();
    let closure_edge = edges
        .iter()
        .find(|edge| {
            edge["chain_id"] == "lifecycle-record-chain-production-v1"
                && edge["from"] == "mainline_merged"
                && edge["to"] == "collab_live_closed"
        })
        .expect("mainline merge must have a live closure edge");
    assert_eq!(closure_edge["owner"], "appsdk::merge_queue");
    assert_eq!(closure_edge["caller"], "lifecycle_chain_promotion");
    assert_eq!(closure_edge["callee"], "assert_collab_live_closure");

    let promotion_edge = edges
        .iter()
        .find(|edge| {
            edge["chain_id"] == "collab-live-closure-v1"
                && edge["from"] == "collab_live_closed"
                && edge["to"] == "promotion_recorded"
        })
        .expect("live closure must precede promotion record production");
    assert_eq!(promotion_edge["caller"], "lifecycle_chain_promotion");
    assert_eq!(promotion_edge["callee"], "lifecycle_chain_write_record");
}
