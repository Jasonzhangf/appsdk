//! Optional executable ontology governance for DAGPipe consumers.
//!
//! The declaration parser in this module validates the execution extension of
//! an ontology manifest. `bind_operation` resolves project operators through
//! the existing `Registry`, protects the commit node, and returns an operation
//! that runs the normal DAG with a per-call commit context.

use crate::{
    compile, graph_topology, Cancellation, CompileError, CompiledGraph, EffectReplay,
    ExecutionFailure, ExecutionResult, Graph, Identity, IteratorKind, Operator, OperatorContext,
    Registry, Runtime, Selector, ValueType,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};

pub const ONTOLOGY_DECLARATION_INVALID: &str = "ONTOLOGY_DECLARATION_INVALID";
pub const ONTOLOGY_REFERENCE_UNRESOLVED: &str = "ONTOLOGY_REFERENCE_UNRESOLVED";
pub const ONTOLOGY_GATE_UNRESOLVED: &str = "ONTOLOGY_GATE_UNRESOLVED";
pub const ONTOLOGY_GRAPH_BYPASS: &str = "ONTOLOGY_GRAPH_BYPASS";
pub const ONTOLOGY_CALL_CONTEXT_MISSING: &str = "ONTOLOGY_CALL_CONTEXT_MISSING";
pub const ONTOLOGY_TRANSITION_REJECTED: &str = "ONTOLOGY_TRANSITION_REJECTED";
pub const ONTOLOGY_OBJECT_REJECTED: &str = "ONTOLOGY_OBJECT_REJECTED";
pub const ONTOLOGY_RELATION_REJECTED: &str = "ONTOLOGY_RELATION_REJECTED";
pub const ONTOLOGY_COMMIT_CONFLICT: &str = "ONTOLOGY_COMMIT_CONFLICT";
pub const ONTOLOGY_OWNER_FAILED: &str = "ONTOLOGY_OWNER_FAILED";

const COMMIT_OPERATOR: &str = "governance.commit";
const COMMIT_OPERATOR_VERSION: &str = "1";
const COMMIT_EFFECT: &str = "ontology.commit";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subject {
    pub object_type: String,
    pub scope: String,
    pub id: String,
}

#[derive(Clone, Debug)]
pub struct OperationRequest {
    pub subject: Subject,
    pub event: String,
    pub data: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GovernanceSnapshot {
    pub objects: Vec<Value>,
    pub relations: Vec<Value>,
}

#[derive(Clone, Debug)]
pub struct Invocation {
    subject: Subject,
    event: String,
}

impl Invocation {
    pub fn subject(&self) -> &Subject {
        &self.subject
    }

    pub fn event(&self) -> &str {
        &self.event
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernanceError {
    pub code: &'static str,
    pub message: String,
    pub clause_id: Option<String>,
    pub operation_id: Option<String>,
}

impl GovernanceError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            clause_id: None,
            operation_id: None,
        }
    }

    fn with_clause(mut self, clause_id: impl Into<String>) -> Self {
        self.clause_id = Some(clause_id.into());
        self
    }

    fn with_operation(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    pub fn commit_conflict(message: impl Into<String>) -> Self {
        Self::new(ONTOLOGY_COMMIT_CONFLICT, message)
    }

    pub fn owner_failed(message: impl Into<String>) -> Self {
        Self::new(ONTOLOGY_OWNER_FAILED, message)
    }
}

impl fmt::Display for GovernanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for GovernanceError {}

#[derive(Clone, Debug)]
pub enum OperationFailure {
    Admission(GovernanceError),
    Execution(ExecutionFailure),
}

impl fmt::Display for OperationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admission(error) => error.fmt(f),
            Self::Execution(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for OperationFailure {}

pub trait CommitOwner: Send + Sync {
    fn apply(
        &self,
        invocation: &Invocation,
        prepared: Value,
        validate: &mut dyn FnMut(
            &GovernanceSnapshot,
            &GovernanceSnapshot,
        ) -> Result<(), GovernanceError>,
    ) -> Result<Value, GovernanceError>;
}

#[derive(Clone, Deserialize)]
struct DeclaredOntology {
    schema_version: u32,
    objects: Vec<DeclaredObject>,
    behaviors: Vec<DeclaredBehavior>,
    relations: Vec<DeclaredRelation>,
    clauses: Vec<DeclaredClause>,
    execution: Option<ExecutionDeclaration>,
}

#[derive(Clone, Deserialize)]
struct DeclaredObject {
    id: String,
}

#[derive(Clone, Deserialize)]
struct DeclaredBehavior {
    id: String,
    transition: DeclaredTransition,
}

#[derive(Clone, Deserialize)]
struct DeclaredTransition {
    from: String,
    to: String,
}

#[derive(Clone, Deserialize)]
struct DeclaredRelation {
    id: String,
    from: String,
    to: String,
}

#[derive(Clone, Deserialize)]
struct DeclaredClause {
    clause_id: String,
    rule_id: String,
    axis: String,
    axis_id: String,
    approval: DeclaredApproval,
    governance: DeclaredGovernance,
}

#[derive(Clone, Deserialize)]
struct DeclaredApproval {
    state: String,
    source: String,
    text: String,
}

#[derive(Clone, Deserialize)]
struct DeclaredGovernance {
    edge: String,
    check: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionDeclaration {
    version: u32,
    objects: Vec<ExecutionObject>,
    relations: Vec<ExecutionRelation>,
    operations: Vec<ExecutionOperation>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionObject {
    object_id: String,
    states: Vec<String>,
    contract: OperatorBinding,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorBinding {
    operator: String,
    version: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionRelation {
    relation_id: String,
    scope: String,
    forward: Cardinality,
    #[serde(default)]
    reverse: Option<Cardinality>,
    create_by: Vec<String>,
    replace_by: Vec<String>,
    remove_by: Vec<String>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cardinality {
    min: usize,
    max: Option<usize>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionOperation {
    behavior_id: String,
    object_id: String,
    event: String,
    graph_id: String,
    commit_node: String,
    clauses: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Axis {
    Object,
    Behavior,
    Relation,
}

impl Axis {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "object" => Some(Self::Object),
            "behavior" => Some(Self::Behavior),
            "relation" => Some(Self::Relation),
            _ => None,
        }
    }

    fn rejection_code(self) -> &'static str {
        match self {
            Self::Object => ONTOLOGY_OBJECT_REJECTED,
            Self::Behavior => ONTOLOGY_TRANSITION_REJECTED,
            Self::Relation => ONTOLOGY_RELATION_REJECTED,
        }
    }
}

#[derive(Clone)]
struct ObjectBinding {
    states: BTreeSet<String>,
    contract: Arc<dyn Operator>,
}

#[derive(Clone)]
struct RelationBinding {
    from: String,
    to: String,
    scope: RelationScope,
    forward: BoundCardinality,
    reverse: Option<BoundCardinality>,
    create_by: BTreeSet<String>,
    replace_by: BTreeSet<String>,
    remove_by: BTreeSet<String>,
}

#[derive(Clone, Copy)]
enum RelationScope {
    SameScope,
    CrossScope,
}

#[derive(Clone, Copy)]
struct BoundCardinality {
    min: usize,
    max: Option<usize>,
}

#[derive(Clone)]
enum BoundGate {
    Object(String),
    Relation,
    Transition,
    Project {
        axis: Axis,
        axis_id: String,
        check: String,
        operator: Arc<dyn Operator>,
    },
}

#[derive(Clone)]
struct CompiledClause {
    clause_id: String,
    gate: BoundGate,
}

#[derive(Clone)]
struct BoundOperation {
    object_id: String,
    event: String,
    behavior_id: String,
    transition_from: String,
    transition_to: String,
    object_bindings: BTreeMap<String, ObjectBinding>,
    relation_bindings: BTreeMap<String, RelationBinding>,
    clauses: Vec<CompiledClause>,
}

pub struct CompiledOperation {
    graph: CompiledGraph,
    operation: Arc<BoundOperation>,
    owner: Arc<dyn CommitOwner>,
    runtime: Runtime,
    commit_node: String,
    input_arc: String,
}

impl CompiledOperation {
    pub fn call(
        &self,
        request: OperationRequest,
        identity: Identity,
        cancellation: &Cancellation,
    ) -> Result<ExecutionResult, OperationFailure> {
        if request.subject.object_type.is_empty()
            || request.subject.scope.is_empty()
            || request.subject.id.is_empty()
        {
            return Err(OperationFailure::Admission(
                GovernanceError::new(
                    ONTOLOGY_OBJECT_REJECTED,
                    "subject object_type, scope, and id are required",
                )
                .with_operation(self.operation.behavior_id.clone()),
            ));
        }
        if request.subject.object_type != self.operation.object_id
            || request.event != self.operation.event
        {
            return Err(OperationFailure::Admission(
                GovernanceError::new(
                    ONTOLOGY_TRANSITION_REJECTED,
                    format!(
                        "request subject/event does not match bound operation `{}`",
                        self.operation.behavior_id
                    ),
                )
                .with_operation(self.operation.behavior_id.clone()),
            ));
        }

        let invocation = Arc::new(Invocation {
            subject: request.subject,
            event: request.event,
        });
        let mut graph = self.graph.clone();
        let commit = graph.nodes.get_mut(&self.commit_node).ok_or_else(|| {
            OperationFailure::Admission(GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!("compiled commit node `{}` is missing", self.commit_node),
            ))
        })?;
        commit.operator = Arc::new(InvocationCommitOperator {
            invocation,
            operation: self.operation.clone(),
            owner: self.owner.clone(),
        });

        let inputs = HashMap::from([(self.input_arc.clone(), request.data)]);
        self.runtime
            .run(&graph, identity, inputs, cancellation)
            .map_err(OperationFailure::Execution)
    }
}

pub fn validate_execution_declaration(ontology: &Value) -> Result<(), GovernanceError> {
    let declared = parse_ontology(ontology)?;
    if declared.execution.is_none() {
        return Ok(());
    }
    validate_execution_models(&declared)?;
    Ok(())
}

pub fn bind_operation(
    ontology: &Value,
    behavior_id: &str,
    graph: Graph,
    registry: Registry,
    owner: Arc<dyn CommitOwner>,
    capabilities: &BTreeSet<String>,
) -> Result<CompiledOperation, GovernanceError> {
    let declared = parse_ontology(ontology)?;
    validate_execution_models(&declared)?;
    let execution = declared.execution.as_ref().ok_or_else(|| {
        GovernanceError::new(
            ONTOLOGY_DECLARATION_INVALID,
            "ontology execution extension is required for bind_operation",
        )
    })?;
    let operation = execution
        .operations
        .iter()
        .find(|operation| operation.behavior_id == behavior_id)
        .ok_or_else(|| {
            GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!("operation behavior `{behavior_id}` is not declared"),
            )
        })?;
    let behavior = declared
        .behaviors
        .iter()
        .find(|behavior| behavior.id == behavior_id)
        .ok_or_else(|| {
            GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!("behavior `{behavior_id}` is not declared"),
            )
        })?;

    ensure_graph_protection(&graph, &registry, operation, &declared)?;
    let bound_operation = bind_bound_operation(&declared, operation, behavior, &registry)?;

    let mut registry = registry;
    registry
        .register(TemplateCommitOperator)
        .map_err(|error| declared_invalid(error))?;
    let compiled_graph = compile(graph, &registry, capabilities).map_err(|error| {
        GovernanceError::new(
            ONTOLOGY_DECLARATION_INVALID,
            format!("governed graph could not be compiled: {error}"),
        )
    })?;
    let input_arc = compiled_graph
        .inputs
        .first()
        .ok_or_else(|| {
            GovernanceError::new(ONTOLOGY_GRAPH_BYPASS, "governed graph has no input ARC")
        })?
        .id
        .clone();
    Ok(CompiledOperation {
        graph: compiled_graph,
        operation: Arc::new(bound_operation),
        owner,
        runtime: Runtime::new(capabilities.clone()),
        commit_node: operation.commit_node.clone(),
        input_arc,
    })
}

struct TemplateCommitOperator;

impl Operator for TemplateCommitOperator {
    fn name(&self) -> &'static str {
        COMMIT_OPERATOR
    }

    fn version(&self) -> &'static str {
        COMMIT_OPERATOR_VERSION
    }

    fn input_type(&self) -> ValueType {
        ValueType::Object
    }

    fn output_type(&self) -> ValueType {
        ValueType::Object
    }

    fn effects(&self) -> &'static [&'static str] {
        &[COMMIT_EFFECT]
    }

    fn replay(&self) -> EffectReplay {
        EffectReplay::RequiresConfirmation
    }

    fn execute(&self, _: Value, _: &OperatorContext) -> Result<Value, String> {
        Err(format!(
            "{ONTOLOGY_CALL_CONTEXT_MISSING}: commit node requires a per-call invocation"
        ))
    }
}

struct InvocationCommitOperator {
    invocation: Arc<Invocation>,
    operation: Arc<BoundOperation>,
    owner: Arc<dyn CommitOwner>,
}

impl Operator for InvocationCommitOperator {
    fn name(&self) -> &'static str {
        COMMIT_OPERATOR
    }

    fn version(&self) -> &'static str {
        COMMIT_OPERATOR_VERSION
    }

    fn input_type(&self) -> ValueType {
        ValueType::Object
    }

    fn output_type(&self) -> ValueType {
        ValueType::Object
    }

    fn effects(&self) -> &'static [&'static str] {
        &[COMMIT_EFFECT]
    }

    fn replay(&self) -> EffectReplay {
        EffectReplay::RequiresConfirmation
    }

    fn execute(&self, prepared: Value, context: &OperatorContext) -> Result<Value, String> {
        let mut validate = |current: &GovernanceSnapshot,
                            candidate: &GovernanceSnapshot|
         -> Result<(), GovernanceError> {
            self.operation
                .validate(&self.invocation, current, candidate, context)
        };
        self.owner
            .apply(&self.invocation, prepared, &mut validate)
            .map_err(|error| error.to_string())
    }
}

impl BoundOperation {
    fn validate(
        &self,
        invocation: &Invocation,
        current: &GovernanceSnapshot,
        candidate: &GovernanceSnapshot,
        context: &OperatorContext,
    ) -> Result<(), GovernanceError> {
        let current_objects = index_objects(&current.objects, self)?;
        let candidate_objects = index_objects(&candidate.objects, self)?;
        if !current_objects.keys().eq(candidate_objects.keys()) {
            return Err(object_rejected(
                "candidate object identities must match the current snapshot",
                self,
            ));
        }
        let subject_key = object_key(
            &invocation.subject.object_type,
            &invocation.subject.scope,
            &invocation.subject.id,
        );
        let current_subject = current_objects.get(&subject_key).ok_or_else(|| {
            GovernanceError::new(
                ONTOLOGY_OBJECT_REJECTED,
                "current snapshot does not contain the operation subject",
            )
            .with_operation(self.behavior_id.clone())
        })?;
        let candidate_subject = candidate_objects.get(&subject_key).ok_or_else(|| {
            GovernanceError::new(
                ONTOLOGY_OBJECT_REJECTED,
                "candidate snapshot does not contain the operation subject",
            )
            .with_operation(self.behavior_id.clone())
        })?;
        let current_state = current_subject
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(|| object_rejected("current subject has no string state", self))?;
        let candidate_state = candidate_subject
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(|| object_rejected("candidate subject has no string state", self))?;
        let subject_binding = self
            .object_bindings
            .get(&invocation.subject.object_type)
            .ok_or_else(|| {
                object_rejected(
                    format!(
                        "subject object type `{}` has no execution binding",
                        invocation.subject.object_type
                    ),
                    self,
                )
            })?;
        if !subject_binding.states.contains(current_state) {
            return Err(object_rejected(
                format!("current subject state `{current_state}` is undeclared"),
                self,
            ));
        }
        if !subject_binding.states.contains(candidate_state) {
            return Err(object_rejected(
                format!("candidate subject state `{candidate_state}` is undeclared"),
                self,
            ));
        }
        if current_state != self.transition_from || invocation.event != self.event {
            return Err(GovernanceError::new(
                ONTOLOGY_TRANSITION_REJECTED,
                format!(
                    "event `{}` cannot transition `{}` from `{current_state}` to `{candidate_state}`",
                    invocation.event, self.behavior_id
                ),
            )
            .with_operation(self.behavior_id.clone()));
        }
        if candidate_state != self.transition_to {
            return Err(GovernanceError::new(
                ONTOLOGY_TRANSITION_REJECTED,
                format!(
                    "candidate state `{candidate_state}` does not match `{}`",
                    self.transition_to
                ),
            )
            .with_operation(self.behavior_id.clone()));
        }

        for (key, object) in &candidate_objects {
            let object_type = object
                .get("type")
                .and_then(Value::as_str)
                .expect("object index validates object type");
            let binding = self.object_bindings.get(object_type).ok_or_else(|| {
                object_rejected(
                    format!("object type `{object_type}` has no execution binding"),
                    self,
                )
            })?;
            let state = object
                .get("state")
                .and_then(Value::as_str)
                .expect("object index validates object state");
            if !binding.states.contains(state) {
                return Err(object_rejected(
                    format!("object `{key}` uses undeclared state `{state}`"),
                    self,
                ));
            }
            let view = json!({
                "type": object_type,
                "scope": object["scope"],
                "id": object["id"],
                "state": state,
                "data": object["data"],
            });
            execute_boolean_gate(
                &binding.contract,
                view,
                context,
                ONTOLOGY_OBJECT_REJECTED,
                self.clauses.iter().find_map(|clause| match &clause.gate {
                    BoundGate::Object(id) if id == object_type => Some(clause.clause_id.as_str()),
                    _ => None,
                }),
                self,
            )?;
        }

        let candidate_relations = index_relations(&candidate.relations, &candidate_objects, self)?;
        let current_relations = index_relations(&current.relations, &current_objects, self)?;
        validate_relation_cardinality(
            &candidate_relations,
            &candidate_objects,
            &self.relation_bindings,
            self,
        )?;
        validate_relation_changes(
            &current_relations,
            &candidate_relations,
            &self.relation_bindings,
            self,
        )?;

        let view = json!({
            "current": {
                "objects": current.objects,
                "relations": current.relations,
            },
            "candidate": {
                "objects": candidate.objects,
                "relations": candidate.relations,
            },
            "subject": {
                "type": invocation.subject.object_type,
                "scope": invocation.subject.scope,
                "id": invocation.subject.id,
            },
        });

        let mut checked = BTreeSet::new();
        for clause in &self.clauses {
            let BoundGate::Project {
                axis,
                axis_id,
                check,
                operator,
            } = &clause.gate
            else {
                continue;
            };
            let key = (axis_key(*axis), axis_id.as_str(), check.as_str());
            if checked.insert(key) {
                execute_boolean_gate(
                    operator,
                    view.clone(),
                    context,
                    axis.rejection_code(),
                    Some(&clause.clause_id),
                    self,
                )?;
            }
        }
        Ok(())
    }
}

fn axis_key(axis: Axis) -> u8 {
    match axis {
        Axis::Object => 0,
        Axis::Behavior => 1,
        Axis::Relation => 2,
    }
}

fn object_rejected(message: impl Into<String>, operation: &BoundOperation) -> GovernanceError {
    GovernanceError::new(ONTOLOGY_OBJECT_REJECTED, message)
        .with_operation(operation.behavior_id.clone())
}

fn index_objects<'a>(
    objects: &'a [Value],
    operation: &BoundOperation,
) -> Result<BTreeMap<String, &'a Value>, GovernanceError> {
    let mut index = BTreeMap::new();
    for object in objects {
        let object_type = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| object_rejected("object type must be a string", operation))?;
        let scope = object
            .get("scope")
            .and_then(Value::as_str)
            .ok_or_else(|| object_rejected("object scope must be a string", operation))?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| object_rejected("object id must be a string", operation))?;
        if object_type.is_empty() || scope.is_empty() || id.is_empty() {
            return Err(object_rejected(
                "object type, scope, and id must be non-empty",
                operation,
            ));
        }
        if object.get("state").and_then(Value::as_str).is_none()
            || !object.get("data").is_some_and(Value::is_object)
        {
            return Err(object_rejected(
                "object state must be a string and data must be an object",
                operation,
            ));
        }
        let key = object_key(object_type, scope, id);
        if index.insert(key.clone(), object).is_some() {
            return Err(object_rejected(
                format!("duplicate object identity `{key}`"),
                operation,
            ));
        }
    }
    Ok(index)
}

fn object_key(object_type: &str, scope: &str, id: &str) -> String {
    serde_json::to_string(&(object_type, scope, id))
        .expect("serializing a tuple of strings cannot fail")
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Endpoint {
    object_type: String,
    scope: String,
    id: String,
}

impl Endpoint {
    fn parse(value: &Value, operation: &BoundOperation) -> Result<Self, GovernanceError> {
        let object_type = value.get("type").and_then(Value::as_str).ok_or_else(|| {
            relation_rejected("relation endpoint type must be a string", operation)
        })?;
        let scope = value.get("scope").and_then(Value::as_str).ok_or_else(|| {
            relation_rejected("relation endpoint scope must be a string", operation)
        })?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| relation_rejected("relation endpoint id must be a string", operation))?;
        if object_type.is_empty() || scope.is_empty() || id.is_empty() {
            return Err(relation_rejected(
                "relation endpoint identity fields must be non-empty",
                operation,
            ));
        }
        Ok(Self {
            object_type: object_type.into(),
            scope: scope.into(),
            id: id.into(),
        })
    }

    fn key(&self) -> String {
        object_key(&self.object_type, &self.scope, &self.id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct RelationView {
    relation_type: String,
    from: Endpoint,
    to: Endpoint,
}

fn index_relations(
    relations: &[Value],
    objects: &BTreeMap<String, &Value>,
    operation: &BoundOperation,
) -> Result<BTreeSet<RelationView>, GovernanceError> {
    let mut result = BTreeSet::new();
    for relation in relations {
        let relation_type = relation
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| relation_rejected("relation type must be a string", operation))?;
        let from_value = relation
            .get("from")
            .ok_or_else(|| relation_rejected("relation `from` endpoint is required", operation))?;
        let to_value = relation
            .get("to")
            .ok_or_else(|| relation_rejected("relation `to` endpoint is required", operation))?;
        let from = Endpoint::parse(from_value, operation)?;
        let to = Endpoint::parse(to_value, operation)?;
        let binding = operation
            .relation_bindings
            .get(relation_type)
            .ok_or_else(|| {
                relation_rejected(
                    format!("relation type `{relation_type}` has no execution binding"),
                    operation,
                )
            })?;
        if from.object_type != binding.from || to.object_type != binding.to {
            return Err(relation_rejected(
                format!("relation `{relation_type}` endpoint types do not match declaration"),
                operation,
            ));
        }
        if matches!(binding.scope, RelationScope::SameScope) && from.scope != to.scope {
            return Err(relation_rejected(
                format!("relation `{relation_type}` crosses scope"),
                operation,
            ));
        }
        if !objects.contains_key(&from.key()) || !objects.contains_key(&to.key()) {
            return Err(relation_rejected(
                format!("relation `{relation_type}` has a missing endpoint"),
                operation,
            ));
        }
        if !result.insert(RelationView {
            relation_type: relation_type.into(),
            from,
            to,
        }) {
            return Err(relation_rejected(
                format!("duplicate relation `{relation_type}`"),
                operation,
            ));
        }
    }
    Ok(result)
}

fn validate_relation_cardinality(
    relations: &BTreeSet<RelationView>,
    objects: &BTreeMap<String, &Value>,
    bindings: &BTreeMap<String, RelationBinding>,
    operation: &BoundOperation,
) -> Result<(), GovernanceError> {
    for (relation_id, binding) in bindings {
        let matching: Vec<_> = relations
            .iter()
            .filter(|relation| relation.relation_type == *relation_id)
            .collect();
        let mut forward = BTreeMap::<Endpoint, usize>::new();
        let mut reverse = BTreeMap::<Endpoint, usize>::new();
        for relation in matching {
            *forward.entry(relation.from.clone()).or_default() += 1;
            *reverse.entry(relation.to.clone()).or_default() += 1;
        }
        for object in objects.values() {
            let endpoint = Endpoint::parse(object, operation)?;
            if endpoint.object_type == binding.from {
                check_cardinality(
                    forward.get(&endpoint).copied().unwrap_or(0),
                    &binding.forward,
                    format!("relation `{relation_id}` from `{}`", endpoint.key()),
                    operation,
                )?;
            }
        }
        if let Some(cardinality) = binding.reverse {
            for object in objects.values() {
                let endpoint = Endpoint::parse(object, operation)?;
                if endpoint.object_type == binding.to {
                    check_cardinality(
                        reverse.get(&endpoint).copied().unwrap_or(0),
                        &cardinality,
                        format!("relation `{relation_id}` to `{}`", endpoint.key()),
                        operation,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn check_cardinality(
    count: usize,
    cardinality: &BoundCardinality,
    label: String,
    operation: &BoundOperation,
) -> Result<(), GovernanceError> {
    if count < cardinality.min || cardinality.max.is_some_and(|max| count > max) {
        return Err(relation_rejected(
            format!("{label} violates declared cardinality"),
            operation,
        ));
    }
    Ok(())
}

fn validate_relation_changes(
    current: &BTreeSet<RelationView>,
    candidate: &BTreeSet<RelationView>,
    bindings: &BTreeMap<String, RelationBinding>,
    operation: &BoundOperation,
) -> Result<(), GovernanceError> {
    let added: Vec<_> = candidate.difference(current).cloned().collect();
    let removed: Vec<_> = current.difference(candidate).cloned().collect();
    let mut replaced_old = BTreeSet::new();
    let mut replaced_new = BTreeSet::new();
    for old in &removed {
        if let Some(new) = added.iter().find(|new| {
            new.relation_type == old.relation_type
                && new.from == old.from
                && !replaced_new.contains(*new)
        }) {
            let binding = bindings.get(&old.relation_type).expect("relation indexed");
            if !binding.replace_by.contains(&operation.behavior_id) {
                return Err(relation_rejected(
                    format!(
                        "behavior `{}` cannot replace relation `{}`",
                        operation.behavior_id, old.relation_type
                    ),
                    operation,
                ));
            }
            replaced_old.insert(old.clone());
            replaced_new.insert(new.clone());
        }
    }
    for relation in added
        .iter()
        .filter(|relation| !replaced_new.contains(*relation))
    {
        let binding = bindings
            .get(&relation.relation_type)
            .expect("relation indexed");
        if !binding.create_by.contains(&operation.behavior_id) {
            return Err(relation_rejected(
                format!(
                    "behavior `{}` cannot create relation `{}`",
                    operation.behavior_id, relation.relation_type
                ),
                operation,
            ));
        }
    }
    for relation in removed
        .iter()
        .filter(|relation| !replaced_old.contains(*relation))
    {
        let binding = bindings
            .get(&relation.relation_type)
            .expect("relation indexed");
        if !binding.remove_by.contains(&operation.behavior_id) {
            return Err(relation_rejected(
                format!(
                    "behavior `{}` cannot remove relation `{}`",
                    operation.behavior_id, relation.relation_type
                ),
                operation,
            ));
        }
    }
    Ok(())
}

fn relation_rejected(message: impl Into<String>, operation: &BoundOperation) -> GovernanceError {
    GovernanceError::new(ONTOLOGY_RELATION_REJECTED, message)
        .with_operation(operation.behavior_id.clone())
}

fn execute_boolean_gate(
    operator: &Arc<dyn Operator>,
    input: Value,
    context: &OperatorContext,
    error_code: &'static str,
    clause_id: Option<&str>,
    operation: &BoundOperation,
) -> Result<(), GovernanceError> {
    let result =
        catch_unwind(AssertUnwindSafe(|| operator.execute(input, context))).map_err(|panic| {
            gate_error(
                error_code,
                format!(
                    "gate `{}` panicked: {}",
                    operator.name(),
                    panic_message(&*panic)
                ),
                clause_id,
                operation,
            )
        })?;
    let value = result.map_err(|cause| {
        gate_error(
            error_code,
            format!("gate `{}` failed: {cause}", operator.name()),
            clause_id,
            operation,
        )
    })?;
    match value.as_bool() {
        Some(true) => Ok(()),
        Some(false) => Err(gate_error(
            error_code,
            format!("gate `{}` rejected the candidate", operator.name()),
            clause_id,
            operation,
        )),
        None => Err(gate_error(
            error_code,
            format!("gate `{}` returned a non-boolean value", operator.name()),
            clause_id,
            operation,
        )),
    }
}

fn gate_error(
    code: &'static str,
    message: String,
    clause_id: Option<&str>,
    operation: &BoundOperation,
) -> GovernanceError {
    let mut error =
        GovernanceError::new(code, message).with_operation(operation.behavior_id.clone());
    if let Some(clause_id) = clause_id {
        error = error.with_clause(clause_id);
    }
    error
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).into()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".into()
    }
}
fn parse_ontology(ontology: &Value) -> Result<DeclaredOntology, GovernanceError> {
    serde_json::from_value(ontology.clone()).map_err(|error| {
        GovernanceError::new(
            ONTOLOGY_DECLARATION_INVALID,
            format!("invalid ontology declaration: {error}"),
        )
    })
}

fn validate_execution_models(declared: &DeclaredOntology) -> Result<(), GovernanceError> {
    if declared.schema_version != 1 {
        return Err(GovernanceError::new(
            ONTOLOGY_DECLARATION_INVALID,
            "ontology schema_version must be 1",
        ));
    }

    let object_ids = unique_ids(
        declared.objects.iter().map(|item| item.id.as_str()),
        "object",
    )?;
    let behavior_ids = unique_ids(
        declared.behaviors.iter().map(|item| item.id.as_str()),
        "behavior",
    )?;
    let relation_ids = unique_ids(
        declared.relations.iter().map(|item| item.id.as_str()),
        "relation",
    )?;
    let clauses: BTreeMap<_, _> = declared
        .clauses
        .iter()
        .map(|clause| (clause.clause_id.as_str(), clause))
        .collect();
    if clauses.len() != declared.clauses.len() {
        return Err(GovernanceError::new(
            ONTOLOGY_DECLARATION_INVALID,
            "ontology clause ids must be unique",
        ));
    }

    for behavior in &declared.behaviors {
        if behavior.transition.from.is_empty() || behavior.transition.to.is_empty() {
            return Err(GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!("behavior `{}` has an empty transition state", behavior.id),
            ));
        }
    }
    for relation in &declared.relations {
        if !object_ids.contains(relation.from.as_str())
            || !object_ids.contains(relation.to.as_str())
        {
            return Err(GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!(
                    "relation `{}` references an undeclared object endpoint",
                    relation.id
                ),
            ));
        }
    }
    for clause in &declared.clauses {
        if clause.rule_id.is_empty()
            || clause.axis_id.is_empty()
            || clause.approval.state != "approved"
            || clause.approval.source.is_empty()
            || clause.approval.text.is_empty()
        {
            return Err(GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!(
                    "clause `{}` is not approved and fully bound",
                    clause.clause_id
                ),
            ));
        }
        let axis = Axis::parse(&clause.axis).ok_or_else(|| {
            GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!("clause `{}` has an unknown axis", clause.clause_id),
            )
        })?;
        let axis_resolves = match axis {
            Axis::Object => object_ids.contains(clause.axis_id.as_str()),
            Axis::Behavior => behavior_ids.contains(clause.axis_id.as_str()),
            Axis::Relation => relation_ids.contains(clause.axis_id.as_str()),
        };
        if !axis_resolves {
            return Err(GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!(
                    "clause `{}` axis_id `{}` is not declared",
                    clause.clause_id, clause.axis_id
                ),
            ));
        }
        if clause.governance.edge.trim().is_empty() {
            return Err(GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!("clause `{}` has no governance edge", clause.clause_id),
            ));
        }
        let (check_name, check_version) = parse_check(&clause.governance.check).map_err(|_| {
            GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!(
                    "clause `{}` has an invalid gate reference",
                    clause.clause_id
                ),
            )
        })?;
        if let Some(kind) = builtin_gate(&check_name, &check_version) {
            if kind != axis {
                return Err(GovernanceError::new(
                    ONTOLOGY_DECLARATION_INVALID,
                    format!(
                        "clause `{}` built-in gate axis does not match its declared axis",
                        clause.clause_id
                    ),
                ));
            }
        }
    }

    let Some(execution) = &declared.execution else {
        return Ok(());
    };
    if execution.version != 1 {
        return Err(GovernanceError::new(
            ONTOLOGY_DECLARATION_INVALID,
            "execution.version must be 1",
        ));
    }
    if execution.operations.is_empty() {
        return Err(GovernanceError::new(
            ONTOLOGY_DECLARATION_INVALID,
            "execution must declare at least one operation",
        ));
    }

    let execution_object_ids = unique_ids(
        execution.objects.iter().map(|item| item.object_id.as_str()),
        "object",
    )?;
    let execution_relation_ids = unique_ids(
        execution
            .relations
            .iter()
            .map(|item| item.relation_id.as_str()),
        "relation",
    )?;
    for object in &execution.objects {
        if !object_ids.contains(object.object_id.as_str()) {
            return Err(GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!("execution object `{}` is not declared", object.object_id),
            ));
        }
        if object.states.is_empty() || unique_strings(&object.states).is_err() {
            return Err(GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!(
                    "execution object `{}` states are empty or duplicated",
                    object.object_id
                ),
            ));
        }
        validate_operator_binding(&object.contract, "object contract")?;
    }
    for relation in &execution.relations {
        if !relation_ids.contains(relation.relation_id.as_str()) {
            return Err(GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!(
                    "execution relation `{}` is not declared",
                    relation.relation_id
                ),
            ));
        }
        let endpoints = declared
            .relations
            .iter()
            .find(|entry| entry.id == relation.relation_id)
            .expect("declared relation exists after reference validation");
        if !execution_object_ids.contains(endpoints.from.as_str())
            || !execution_object_ids.contains(endpoints.to.as_str())
        {
            return Err(GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!(
                    "relation `{}` endpoint has no execution object binding",
                    relation.relation_id
                ),
            ));
        }
        if !matches!(relation.scope.as_str(), "same_scope" | "cross_scope") {
            return Err(GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!(
                    "execution relation `{}` has an invalid scope",
                    relation.relation_id
                ),
            ));
        }
        validate_cardinality(&relation.forward, "forward")?;
        if let Some(reverse) = &relation.reverse {
            validate_cardinality(reverse, "reverse")?;
        }
        for behavior_id in relation
            .create_by
            .iter()
            .chain(&relation.replace_by)
            .chain(&relation.remove_by)
        {
            if !behavior_ids.contains(behavior_id.as_str()) {
                return Err(GovernanceError::new(
                    ONTOLOGY_REFERENCE_UNRESOLVED,
                    format!(
                        "execution relation `{}` references unknown behavior `{behavior_id}`",
                        relation.relation_id
                    ),
                ));
            }
        }
    }

    unique_ids(
        execution
            .operations
            .iter()
            .map(|operation| operation.behavior_id.as_str()),
        "operation behavior",
    )?;
    let mut operation_keys = BTreeSet::new();
    for operation in &execution.operations {
        if !object_ids.contains(operation.object_id.as_str())
            || !behavior_ids.contains(operation.behavior_id.as_str())
        {
            return Err(GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!(
                    "operation `{}` references an undeclared object or behavior",
                    operation.behavior_id
                ),
            ));
        }
        let behavior = declared
            .behaviors
            .iter()
            .find(|behavior| behavior.id == operation.behavior_id)
            .expect("behavior exists after reference validation");
        let object = execution
            .objects
            .iter()
            .find(|object| object.object_id == operation.object_id)
            .ok_or_else(|| {
                GovernanceError::new(
                    ONTOLOGY_REFERENCE_UNRESOLVED,
                    format!(
                        "operation `{}` has no execution object definition",
                        operation.behavior_id
                    ),
                )
            })?;
        if !object.states.contains(&behavior.transition.from)
            || !object.states.contains(&behavior.transition.to)
        {
            return Err(GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!(
                    "operation `{}` transition uses an undeclared state",
                    operation.behavior_id
                ),
            ));
        }
        if operation.event.is_empty()
            || operation.graph_id.is_empty()
            || operation.commit_node.is_empty()
        {
            return Err(GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!(
                    "operation `{}` has an empty execution field",
                    operation.behavior_id
                ),
            ));
        }
        if !operation_keys.insert((
            operation.object_id.clone(),
            behavior.transition.from.clone(),
            operation.event.clone(),
        )) {
            return Err(GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!(
                    "operation `{}` duplicates object/state/event dispatch",
                    operation.behavior_id
                ),
            ));
        }
        if operation.clauses.is_empty() || unique_strings(&operation.clauses).is_err() {
            return Err(GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!(
                    "operation `{}` clauses are empty or duplicated",
                    operation.behavior_id
                ),
            ));
        }
        for clause_id in &operation.clauses {
            let clause = clauses.get(clause_id.as_str()).ok_or_else(|| {
                GovernanceError::new(
                    ONTOLOGY_REFERENCE_UNRESOLVED,
                    format!(
                        "operation `{}` references unknown clause `{clause_id}`",
                        operation.behavior_id
                    ),
                )
            })?;
            let (check_name, check_version) =
                parse_check(&clause.governance.check).map_err(|_| {
                    GovernanceError::new(
                        ONTOLOGY_DECLARATION_INVALID,
                        format!("clause `{clause_id}` has an invalid gate reference"),
                    )
                })?;
            if let Some(kind) = builtin_gate(&check_name, &check_version) {
                match kind {
                    Axis::Object if !execution_object_ids.contains(clause.axis_id.as_str()) => {
                        return Err(GovernanceError::new(
                            ONTOLOGY_REFERENCE_UNRESOLVED,
                            format!("object clause `{clause_id}` has no execution binding"),
                        ));
                    }
                    Axis::Relation if !execution_relation_ids.contains(clause.axis_id.as_str()) => {
                        return Err(GovernanceError::new(
                            ONTOLOGY_REFERENCE_UNRESOLVED,
                            format!("relation clause `{clause_id}` has no execution binding"),
                        ));
                    }
                    Axis::Behavior if clause.axis_id != operation.behavior_id => {
                        return Err(GovernanceError::new(
                            ONTOLOGY_DECLARATION_INVALID,
                            format!(
                                "transition clause `{clause_id}` does not bind operation `{}`",
                                operation.behavior_id
                            ),
                        ));
                    }
                    Axis::Object | Axis::Behavior | Axis::Relation => {}
                }
            }
            if let Some((edge_graph, edge_from, edge_to)) = parse_edge_ref(&clause.governance.edge)
            {
                if edge_graph != operation.graph_id || edge_to != operation.commit_node {
                    return Err(GovernanceError::new(
                        ONTOLOGY_REFERENCE_UNRESOLVED,
                        format!(
                            "clause `{clause_id}` edge does not enter operation `{}` commit node",
                            operation.behavior_id
                        ),
                    ));
                }
                if edge_from.is_empty() {
                    return Err(GovernanceError::new(
                        ONTOLOGY_DECLARATION_INVALID,
                        format!("clause `{clause_id}` edge has an empty source"),
                    ));
                }
            } else {
                return Err(GovernanceError::new(
                    ONTOLOGY_DECLARATION_INVALID,
                    format!("clause `{clause_id}` edge has an invalid format"),
                ));
            }
        }
    }
    Ok(())
}

fn unique_ids<'a>(
    ids: impl Iterator<Item = &'a str>,
    label: &str,
) -> Result<BTreeSet<&'a str>, GovernanceError> {
    let mut result = BTreeSet::new();
    for id in ids {
        if id.is_empty() || !result.insert(id) {
            return Err(GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!("{label} ids must be non-empty and unique"),
            ));
        }
    }
    Ok(result)
}

fn unique_strings(values: &[String]) -> Result<(), ()> {
    let mut seen = BTreeSet::new();
    if values
        .iter()
        .all(|value| !value.is_empty() && seen.insert(value))
    {
        Ok(())
    } else {
        Err(())
    }
}

fn validate_operator_binding(
    binding: &OperatorBinding,
    label: &str,
) -> Result<(), GovernanceError> {
    if binding.operator.is_empty() || binding.version.is_empty() {
        return Err(GovernanceError::new(
            ONTOLOGY_DECLARATION_INVALID,
            format!("{label} operator and version are required"),
        ));
    }
    Ok(())
}

fn validate_cardinality(cardinality: &Cardinality, label: &str) -> Result<(), GovernanceError> {
    if cardinality.max.is_some_and(|max| max < cardinality.min) {
        return Err(GovernanceError::new(
            ONTOLOGY_DECLARATION_INVALID,
            format!("{label} cardinality minimum exceeds maximum"),
        ));
    }
    Ok(())
}

fn parse_check(check: &str) -> Result<(String, String), ()> {
    let (name, version) = check.rsplit_once('@').ok_or(())?;
    if name.is_empty() || version.is_empty() {
        return Err(());
    }
    Ok((name.into(), version.into()))
}

fn builtin_gate(name: &str, version: &str) -> Option<Axis> {
    match (name, version) {
        ("ontology.object", "1") => Some(Axis::Object),
        ("ontology.behavior", "1") | ("ontology.transition", "1") => Some(Axis::Behavior),
        ("ontology.relation", "1") => Some(Axis::Relation),
        _ => None,
    }
}

fn parse_edge_ref(edge: &str) -> Option<(String, String, String)> {
    let (graph_id, rest) = edge.split_once('/')?;
    let (from, to) = rest.split_once("->")?;
    if graph_id.is_empty() || from.is_empty() || to.is_empty() {
        return None;
    }
    Some((graph_id.into(), from.into(), to.into()))
}

fn declared_invalid(error: CompileError) -> GovernanceError {
    GovernanceError::new(
        ONTOLOGY_DECLARATION_INVALID,
        format!("invalid governed declaration: {error}"),
    )
}

fn ensure_graph_protection(
    graph: &Graph,
    registry: &Registry,
    operation: &ExecutionOperation,
    declared: &DeclaredOntology,
) -> Result<(), GovernanceError> {
    if graph.id != operation.graph_id {
        return Err(GovernanceError::new(
            ONTOLOGY_REFERENCE_UNRESOLVED,
            format!(
                "operation `{}` expects graph `{}`, got `{}`",
                operation.behavior_id, operation.graph_id, graph.id
            ),
        ));
    }
    graph_topology(graph).map_err(|error| {
        GovernanceError::new(
            ONTOLOGY_GRAPH_BYPASS,
            format!("governed graph topology is invalid: {error}"),
        )
    })?;

    for node in &graph.nodes {
        if node.iterator != IteratorKind::Whole
            || !selector_is_unfiltered(&node.input_selector)
            || !selector_is_unfiltered(&node.output_selector)
        {
            return Err(GovernanceError::new(
                ONTOLOGY_GRAPH_BYPASS,
                format!(
                    "node `{}` uses filtering or item iteration on a governed protection path",
                    node.id
                ),
            ));
        }
    }

    let commit = graph
        .nodes
        .iter()
        .find(|node| node.id == operation.commit_node)
        .ok_or_else(|| {
            GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!(
                    "operation `{}` commit node `{}` does not exist",
                    operation.behavior_id, operation.commit_node
                ),
            )
        })?;
    if commit.operator != COMMIT_OPERATOR || commit.operator_version != COMMIT_OPERATOR_VERSION {
        return Err(GovernanceError::new(
            ONTOLOGY_GRAPH_BYPASS,
            format!(
                "commit node `{}` must bind `{COMMIT_OPERATOR}@{COMMIT_OPERATOR_VERSION}`",
                commit.id
            ),
        ));
    }
    if registry
        .operators
        .contains_key(&(COMMIT_OPERATOR.into(), COMMIT_OPERATOR_VERSION.into()))
    {
        return Err(GovernanceError::new(
            ONTOLOGY_DECLARATION_INVALID,
            "project registry conflicts with the SDK protected commit operator",
        ));
    }

    for node in &graph.nodes {
        if node.id == operation.commit_node {
            continue;
        }
        let operator = registry
            .operators
            .get(&(node.operator.clone(), node.operator_version.clone()))
            .ok_or_else(|| {
                GovernanceError::new(
                    ONTOLOGY_GATE_UNRESOLVED,
                    format!(
                        "graph node `{}` references missing operator `{}@{}`",
                        node.id, node.operator, node.operator_version
                    ),
                )
            })?;
        if !operator.effects().is_empty() {
            return Err(GovernanceError::new(
                ONTOLOGY_GRAPH_BYPASS,
                format!(
                    "graph node `{}` declares effects outside the protected commit",
                    node.id
                ),
            ));
        }
    }

    if commit_dominates_output(graph, &operation.commit_node) {
        // The commit node must be on every path from the single input source
        // to the single output and must be reachable from that source.
    } else {
        return Err(GovernanceError::new(
            ONTOLOGY_GRAPH_BYPASS,
            format!(
                "commit node `{}` does not protect every input-to-output path",
                operation.commit_node
            ),
        ));
    }

    let clauses: BTreeMap<_, _> = declared
        .clauses
        .iter()
        .map(|clause| (clause.clause_id.as_str(), clause))
        .collect();
    for clause_id in &operation.clauses {
        let clause = clauses.get(clause_id.as_str()).ok_or_else(|| {
            GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!(
                    "operation `{}` clause `{clause_id}` is missing",
                    operation.behavior_id
                ),
            )
        })?;
        let (edge_graph, edge_from, edge_to) =
            parse_edge_ref(&clause.governance.edge).ok_or_else(|| {
                GovernanceError::new(
                    ONTOLOGY_DECLARATION_INVALID,
                    format!("clause `{}` edge has an invalid format", clause.clause_id),
                )
            })?;
        if edge_graph != graph.id
            || edge_to != operation.commit_node
            || !graph
                .edges
                .iter()
                .any(|edge| edge.from == edge_from && edge.to == operation.commit_node)
        {
            return Err(GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!(
                    "clause `{}` does not bind a real incoming commit edge",
                    clause.clause_id
                ),
            ));
        }
    }
    Ok(())
}

fn selector_is_unfiltered(selector: &Selector) -> bool {
    selector.include.is_empty() && selector.exclude.is_empty() && selector.predicate.is_none()
}

fn commit_dominates_output(graph: &Graph, commit_node: &str) -> bool {
    let source_arc = match graph.inputs.as_slice() {
        [source] => source,
        _ => return false,
    };
    let sources: Vec<String> = graph
        .nodes
        .iter()
        .filter(|node| node.inputs.iter().any(|input| input == &source_arc.id))
        .map(|node| node.id.clone())
        .collect();
    if sources.is_empty() {
        return false;
    }
    let outputs: BTreeSet<String> = graph.outputs.iter().cloned().collect();
    let mut adjacency = BTreeMap::<String, Vec<String>>::new();
    for edge in &graph.edges {
        adjacency
            .entry(edge.from.clone())
            .or_default()
            .push(edge.to.clone());
    }
    let can_reach_commit = reachable_from(&sources, &adjacency, None)
        .iter()
        .any(|node| node == commit_node);
    if !can_reach_commit {
        return false;
    }
    let reachable_without_commit = reachable_from(&sources, &adjacency, Some(commit_node));
    !graph.nodes.iter().any(|node| {
        outputs.contains(&node.output.id) && reachable_without_commit.contains(&node.id)
    })
}

fn reachable_from(
    sources: &[String],
    adjacency: &BTreeMap<String, Vec<String>>,
    blocked: Option<&str>,
) -> BTreeSet<String> {
    let mut reachable = BTreeSet::new();
    let mut pending: Vec<String> = sources.to_vec();
    while let Some(node) = pending.pop() {
        if Some(node.as_str()) == blocked || !reachable.insert(node.clone()) {
            continue;
        }
        if let Some(next) = adjacency.get(&node) {
            pending.extend(next.iter().cloned());
        }
    }
    reachable
}

fn bind_bound_operation(
    declared: &DeclaredOntology,
    operation: &ExecutionOperation,
    behavior: &DeclaredBehavior,
    registry: &Registry,
) -> Result<BoundOperation, GovernanceError> {
    let execution = declared.execution.as_ref().ok_or_else(|| {
        GovernanceError::new(
            ONTOLOGY_DECLARATION_INVALID,
            "ontology execution extension is required",
        )
    })?;
    let clauses: BTreeMap<_, _> = declared
        .clauses
        .iter()
        .map(|clause| (clause.clause_id.as_str(), clause))
        .collect();

    let mut object_bindings = BTreeMap::new();
    for object in &execution.objects {
        let contract = resolve_project_operator(registry, &object.contract, "object contract")?;
        object_bindings.insert(
            object.object_id.clone(),
            ObjectBinding {
                states: object.states.iter().cloned().collect(),
                contract,
            },
        );
    }

    let relations: BTreeMap<_, _> = declared
        .relations
        .iter()
        .map(|relation| (relation.id.as_str(), relation))
        .collect();
    let mut relation_bindings = BTreeMap::new();
    for relation in &execution.relations {
        let declared_relation = relations
            .get(relation.relation_id.as_str())
            .ok_or_else(|| {
                GovernanceError::new(
                    ONTOLOGY_REFERENCE_UNRESOLVED,
                    format!("relation `{}` is not declared", relation.relation_id),
                )
            })?;
        relation_bindings.insert(
            relation.relation_id.clone(),
            RelationBinding {
                from: declared_relation.from.clone(),
                to: declared_relation.to.clone(),
                scope: if relation.scope == "same_scope" {
                    RelationScope::SameScope
                } else {
                    RelationScope::CrossScope
                },
                forward: bound_cardinality(&relation.forward),
                reverse: relation.reverse.as_ref().map(bound_cardinality),
                create_by: relation.create_by.iter().cloned().collect(),
                replace_by: relation.replace_by.iter().cloned().collect(),
                remove_by: relation.remove_by.iter().cloned().collect(),
            },
        );
    }

    let mut compiled_clauses = Vec::with_capacity(operation.clauses.len());
    for clause_id in &operation.clauses {
        let clause = clauses.get(clause_id.as_str()).ok_or_else(|| {
            GovernanceError::new(
                ONTOLOGY_REFERENCE_UNRESOLVED,
                format!("operation clause `{clause_id}` is not declared"),
            )
        })?;
        let axis = Axis::parse(&clause.axis).ok_or_else(|| {
            GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!("clause `{clause_id}` has an unknown axis"),
            )
        })?;
        let (check_name, check_version) = parse_check(&clause.governance.check).map_err(|_| {
            GovernanceError::new(
                ONTOLOGY_DECLARATION_INVALID,
                format!("clause `{clause_id}` has an invalid gate reference"),
            )
        })?;
        let gate = if let Some(builtin_axis) = builtin_gate(&check_name, &check_version) {
            if builtin_axis != axis {
                return Err(GovernanceError::new(
                    ONTOLOGY_DECLARATION_INVALID,
                    format!("clause `{clause_id}` built-in gate axis mismatch"),
                ));
            }
            if registry
                .operators
                .contains_key(&(check_name.clone(), check_version.clone()))
            {
                return Err(GovernanceError::new(
                    ONTOLOGY_DECLARATION_INVALID,
                    format!("project registry conflicts with built-in gate `{check_name}@{check_version}`"),
                ));
            }
            match builtin_axis {
                Axis::Object => BoundGate::Object(clause.axis_id.clone()),
                Axis::Relation => BoundGate::Relation,
                Axis::Behavior => {
                    if clause.axis_id != operation.behavior_id {
                        return Err(GovernanceError::new(
                            ONTOLOGY_DECLARATION_INVALID,
                            format!(
                                "transition clause `{clause_id}` does not bind operation `{}`",
                                operation.behavior_id
                            ),
                        ));
                    }
                    BoundGate::Transition
                }
            }
        } else {
            if check_name.starts_with("ontology.") {
                return Err(GovernanceError::new(
                    ONTOLOGY_GATE_UNRESOLVED,
                    format!("built-in gate `{check_name}@{check_version}` is unresolved"),
                ));
            }
            let operator = resolve_project_gate(registry, &check_name, &check_version)?;
            BoundGate::Project {
                axis,
                axis_id: clause.axis_id.clone(),
                check: format!("{check_name}@{check_version}"),
                operator,
            }
        };
        compiled_clauses.push(CompiledClause {
            clause_id: clause_id.clone(),
            gate,
        });
    }

    Ok(BoundOperation {
        object_id: operation.object_id.clone(),
        event: operation.event.clone(),
        behavior_id: operation.behavior_id.clone(),
        transition_from: behavior.transition.from.clone(),
        transition_to: behavior.transition.to.clone(),
        object_bindings,
        relation_bindings,
        clauses: compiled_clauses,
    })
}

fn bound_cardinality(cardinality: &Cardinality) -> BoundCardinality {
    BoundCardinality {
        min: cardinality.min,
        max: cardinality.max,
    }
}

fn resolve_project_operator(
    registry: &Registry,
    binding: &OperatorBinding,
    label: &str,
) -> Result<Arc<dyn Operator>, GovernanceError> {
    let operator = registry
        .operators
        .get(&(binding.operator.clone(), binding.version.clone()))
        .cloned()
        .ok_or_else(|| {
            GovernanceError::new(
                ONTOLOGY_GATE_UNRESOLVED,
                format!(
                    "{label} `{}@{}` is not registered",
                    binding.operator, binding.version
                ),
            )
        })?;
    validate_project_operator_contract(&operator, label, &binding.operator)?;
    Ok(operator)
}

fn resolve_project_gate(
    registry: &Registry,
    name: &str,
    version: &str,
) -> Result<Arc<dyn Operator>, GovernanceError> {
    let operator = registry
        .operators
        .get(&(name.to_owned(), version.to_owned()))
        .cloned()
        .ok_or_else(|| {
            GovernanceError::new(
                ONTOLOGY_GATE_UNRESOLVED,
                format!("project gate `{name}@{version}` is not registered"),
            )
        })?;
    validate_project_operator_contract(&operator, "project gate", name)?;
    Ok(operator)
}

fn validate_project_operator_contract(
    operator: &Arc<dyn Operator>,
    label: &str,
    name: &str,
) -> Result<(), GovernanceError> {
    let input_type = catch_unwind(AssertUnwindSafe(|| operator.input_type()))
        .map_err(|_| declaration_contract_panic(label, name))?;
    let output_type = catch_unwind(AssertUnwindSafe(|| operator.output_type()))
        .map_err(|_| declaration_contract_panic(label, name))?;
    let effects = catch_unwind(AssertUnwindSafe(|| operator.effects()))
        .map_err(|_| declaration_contract_panic(label, name))?;
    if input_type != ValueType::Object || output_type != ValueType::Boolean || !effects.is_empty() {
        return Err(GovernanceError::new(
            ONTOLOGY_DECLARATION_INVALID,
            format!("{label} `{name}` must accept Object, return Bool, and declare no effects"),
        ));
    }
    Ok(())
}

fn declaration_contract_panic(label: &str, name: &str) -> GovernanceError {
    GovernanceError::new(
        ONTOLOGY_DECLARATION_INVALID,
        format!("{label} `{name}` panicked while declaring its operator contract"),
    )
}
