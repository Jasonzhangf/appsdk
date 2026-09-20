use jsonschema::validator_for;
use serde_json::{json, Value};

const SCHEMA_SOURCE: &str =
    include_str!("../../contracts/records/collab-live-closure-record.schema.json");
const PATHS: [&str; 7] = [
    "peer_to_peer",
    "peer_to_master",
    "master_to_peer",
    "master_to_master",
    "daemon_to_peer",
    "daemon_to_master",
    "restart_replay",
];

fn validator() -> jsonschema::Validator {
    let schema: Value =
        serde_json::from_str(SCHEMA_SOURCE).expect("collab live closure schema must parse");
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
            "native_thread_id": "thread-1",
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
            "resolved_at": "2026-01-01T00:00:00Z",
            "source": "collab_cli"
        },
        "evidence_ids": Value::Object(evidence_ids),
        "path_receipts": Value::Object(path_receipts),
        "created_at": "2026-01-01T00:00:00Z"
    })
}

#[test]
fn schema_accepts_path_identity_bound_receipts() {
    assert!(validator().is_valid(&closure()));
}

#[test]
fn schema_rejects_direction_mismatch_for_every_path() {
    let validator = validator();
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
