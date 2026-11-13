use pipeline_runtime::governance::{
    bind_operation, CommitOwner, CompiledOperation, GovernanceError, GovernanceSnapshot,
    Invocation, OperationFailure, OperationRequest, Subject,
};
use pipeline_runtime::{
    parse_graph_json, Cancellation, Graph, Identity, Operator, OperatorContext, Registry, ValueType,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub fn ontology() -> Value {
    serde_json::from_str::<Value>(include_str!(
        "../fixtures/ontology-governance/catalog-guidance.json"
    ))
    .unwrap()["ontology"]
        .clone()
}

pub fn graph() -> Graph {
    parse_graph_json(include_str!(
        "../fixtures/ontology-governance/activate.graph.json"
    ))
    .unwrap()
}

pub fn initial() -> GovernanceSnapshot {
    let value: Value = serde_json::from_str(include_str!(
        "../fixtures/ontology-governance/consumer/initial-catalog.json"
    ))
    .unwrap();
    GovernanceSnapshot {
        objects: value["objects"].as_array().unwrap().clone(),
        relations: value["relations"].as_array().unwrap().clone(),
    }
}

pub fn candidate() -> GovernanceSnapshot {
    let mut snapshot = initial();
    snapshot.objects[0]["state"] = json!("active");
    snapshot.objects[0]["data"]["title"] = json!("Updated title");
    snapshot.relations.push(json!({
        "type": "record-group",
        "from": {"type": "record", "scope": "demo", "id": "r1"},
        "to": {"type": "group", "scope": "demo", "id": "g1"}
    }));
    snapshot
}

pub struct SnapshotOwner {
    state: Mutex<GovernanceSnapshot>,
    pub commits: AtomicUsize,
}

impl SnapshotOwner {
    pub fn new(snapshot: GovernanceSnapshot) -> Self {
        Self {
            state: Mutex::new(snapshot),
            commits: AtomicUsize::new(0),
        }
    }
    pub fn snapshot(&self) -> GovernanceSnapshot {
        self.state.lock().unwrap().clone()
    }
}

impl CommitOwner for SnapshotOwner {
    fn apply(
        &self,
        invocation: &Invocation,
        prepared: Value,
        validate: &mut dyn FnMut(
            &GovernanceSnapshot,
            &GovernanceSnapshot,
        ) -> Result<(), GovernanceError>,
    ) -> Result<Value, GovernanceError> {
        assert_eq!(invocation.subject().id, "r1");
        assert_eq!(invocation.event(), "activate");
        let mut current = self.state.lock().unwrap();
        let candidate = GovernanceSnapshot {
            objects: prepared["objects"].as_array().unwrap().clone(),
            relations: prepared["relations"].as_array().unwrap().clone(),
        };
        validate(&current, &candidate)?;
        *current = candidate;
        self.commits.fetch_add(1, Ordering::SeqCst);
        Ok(prepared)
    }
}

struct CopyOperator(&'static str);

impl Operator for CopyOperator {
    fn name(&self) -> &'static str {
        self.0
    }
    fn version(&self) -> &'static str {
        "1"
    }
    fn input_type(&self) -> ValueType {
        ValueType::Object
    }
    fn output_type(&self) -> ValueType {
        ValueType::Object
    }
    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        Ok(input)
    }
}

struct ObjectContract(&'static str, &'static str);

impl Operator for ObjectContract {
    fn name(&self) -> &'static str {
        self.0
    }
    fn version(&self) -> &'static str {
        "1"
    }
    fn input_type(&self) -> ValueType {
        ValueType::Object
    }
    fn output_type(&self) -> ValueType {
        ValueType::Boolean
    }
    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        Ok(json!(input["data"][self.1]
            .as_str()
            .is_some_and(|text| !text.trim().is_empty())))
    }
}

pub fn registry() -> Registry {
    let mut registry = Registry::default();
    registry.register(CopyOperator("catalog.prepare")).unwrap();
    registry.register(CopyOperator("catalog.receipt")).unwrap();
    registry
        .register(ObjectContract("catalog.record_contract", "title"))
        .unwrap();
    registry
        .register(ObjectContract("catalog.group_contract", "name"))
        .unwrap();
    registry
}

pub fn bind(
    ontology: &Value,
    graph: Graph,
    registry: Registry,
    owner: Arc<SnapshotOwner>,
) -> Result<CompiledOperation, GovernanceError> {
    bind_operation(
        ontology,
        "activate",
        graph,
        registry,
        owner,
        &BTreeSet::from(["ontology.commit".into()]),
    )
}

pub fn call(
    operation: &CompiledOperation,
    candidate: &GovernanceSnapshot,
) -> Result<(), OperationFailure> {
    operation
        .call(
            OperationRequest {
                subject: Subject {
                    object_type: "record".into(),
                    scope: "demo".into(),
                    id: "r1".into(),
                },
                event: "activate".into(),
                data: json!({"objects": candidate.objects, "relations": candidate.relations}),
            },
            Identity {
                project_id: "test-catalog".into(),
                graph_id: "catalog-activate".into(),
                graph_version: "1".into(),
                execution_id: "operation-call".into(),
                attempt_id: "attempt".into(),
            },
            &Cancellation::default(),
        )
        .map(|_| ())
}
