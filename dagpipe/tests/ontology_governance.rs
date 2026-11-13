#[path = "support/ontology_governance.rs"]
mod support;

use pipeline_runtime::governance::validate_execution_declaration;
use pipeline_runtime::{Operator, OperatorContext, ValueType};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use support::*;

#[test]
fn protected_commit_rejects_the_unprotected_red_control() {
    let mut illegal = candidate();
    illegal.objects[0]["data"]["title"] = json!("");
    // The unguarded assignment persists the invalid candidate, as in the red control.
    let unprotected = illegal.clone();
    assert_eq!(unprotected.objects[0]["data"]["title"], "");
    let owner = Arc::new(SnapshotOwner::new(initial()));
    let operation = bind(&ontology(), graph(), registry(), owner.clone()).unwrap();
    let error = call(&operation, &illegal).unwrap_err().to_string();
    assert!(error.contains("ONTOLOGY_OBJECT_REJECTED"), "{error}");
    assert_eq!(owner.snapshot(), initial());
    assert_eq!(owner.commits.load(Ordering::SeqCst), 0);
    call(&operation, &candidate()).unwrap();
    assert_eq!(owner.snapshot(), candidate());
    assert_eq!(owner.commits.load(Ordering::SeqCst), 1);
}

#[test]
fn unresolved_gate_and_unprotected_graph_fail_before_commit() {
    let owner = Arc::new(SnapshotOwner::new(initial()));
    let mut declaration = ontology();
    declaration["clauses"][0]["governance"]["check"] = json!("catalog.missing@1");
    let error = bind(&declaration, graph(), registry(), owner.clone())
        .err()
        .unwrap();
    assert_eq!(error.code, "ONTOLOGY_GATE_UNRESOLVED");
    let mut damaged = graph();
    damaged.nodes[1].operator = "catalog.prepare".into();
    let error = bind(&ontology(), damaged, registry(), owner.clone())
        .err()
        .unwrap();
    assert_eq!(error.code, "ONTOLOGY_GRAPH_BYPASS");
    assert_eq!(owner.snapshot(), initial());
    assert_eq!(owner.commits.load(Ordering::SeqCst), 0);
}

#[test]
fn relation_minimum_includes_endpoints_with_no_relations() {
    for direction in ["forward", "reverse"] {
        let mut declaration = ontology();
        declaration["execution"]["relations"][0][direction] = json!({"min": 1, "max": 1});
        let owner = Arc::new(SnapshotOwner::new(initial()));
        let operation = bind(&declaration, graph(), registry(), owner.clone()).unwrap();
        let mut disconnected = candidate();
        disconnected.relations.clear();
        let error = call(&operation, &disconnected).unwrap_err().to_string();
        assert!(
            error.contains("ONTOLOGY_RELATION_REJECTED"),
            "{direction}: {error}"
        );
        assert_eq!(owner.snapshot(), initial());
        call(&operation, &candidate()).unwrap();
        assert_eq!(owner.snapshot(), candidate());
    }
}

#[test]
fn operation_dispatch_distinguishes_source_state() {
    let mut declaration = ontology();
    let mut behavior = declaration["behaviors"][0].clone();
    behavior["id"] = json!("deactivate");
    behavior["transition"] = json!({"from": "active", "to": "draft"});
    declaration["behaviors"]
        .as_array_mut()
        .unwrap()
        .push(behavior);
    let mut operation = declaration["execution"]["operations"][0].clone();
    operation["behavior_id"] = json!("deactivate");
    operation["clauses"] = json!(["record-contract"]);
    declaration["execution"]["operations"]
        .as_array_mut()
        .unwrap()
        .push(operation);
    validate_execution_declaration(&declaration).unwrap();
    let mut ambiguous = declaration.clone();
    ambiguous["execution"]["operations"][1]["behavior_id"] = json!("activate");
    ambiguous["execution"]["operations"][1]["event"] = json!("other");
    assert_eq!(
        validate_execution_declaration(&ambiguous).unwrap_err().code,
        "ONTOLOGY_DECLARATION_INVALID"
    );
    declaration["behaviors"][1]["transition"]["from"] = json!("draft");
    assert_eq!(
        validate_execution_declaration(&declaration)
            .unwrap_err()
            .code,
        "ONTOLOGY_DECLARATION_INVALID"
    );
}

enum GateResult {
    Allow,
    Deny,
    Error,
    Panic,
    NonBoolean,
}

struct ProjectGate {
    result: GateResult,
    declared_output: ValueType,
    effectful: bool,
}

impl Operator for ProjectGate {
    fn name(&self) -> &'static str {
        "catalog.gate"
    }
    fn version(&self) -> &'static str {
        "1"
    }
    fn input_type(&self) -> ValueType {
        ValueType::Object
    }
    fn output_type(&self) -> ValueType {
        self.declared_output.clone()
    }
    fn effects(&self) -> &'static [&'static str] {
        if self.effectful {
            &["catalog.write"]
        } else {
            &[]
        }
    }
    fn execute(&self, input: Value, _: &OperatorContext) -> Result<Value, String> {
        assert_eq!(input["subject"]["id"], "r1");
        match self.result {
            GateResult::Allow => Ok(json!(true)),
            GateResult::Deny => Ok(json!(false)),
            GateResult::Error => Err("project gate error".into()),
            GateResult::Panic => panic!("project gate panic"),
            GateResult::NonBoolean => Ok(json!({"allowed": true})),
        }
    }
}

fn custom_declaration() -> Value {
    let mut declaration = ontology();
    declaration["clauses"][3]["governance"]["check"] = json!("catalog.gate@1");
    declaration
}

#[test]
fn project_gate_false_error_panic_and_wrong_value_preserve_state() {
    for result in [
        GateResult::Deny,
        GateResult::Error,
        GateResult::Panic,
        GateResult::NonBoolean,
    ] {
        let owner = Arc::new(SnapshotOwner::new(initial()));
        let mut registry = registry();
        registry
            .register(ProjectGate {
                result,
                declared_output: ValueType::Boolean,
                effectful: false,
            })
            .unwrap();
        let operation = bind(&custom_declaration(), graph(), registry, owner.clone()).unwrap();
        let error = call(&operation, &candidate()).unwrap_err().to_string();
        assert!(error.contains("ONTOLOGY_TRANSITION_REJECTED"), "{error}");
        assert_eq!(owner.snapshot(), initial());
        assert_eq!(owner.commits.load(Ordering::SeqCst), 0);
    }
    let owner = Arc::new(SnapshotOwner::new(initial()));
    let mut registry = registry();
    registry
        .register(ProjectGate {
            result: GateResult::Allow,
            declared_output: ValueType::Boolean,
            effectful: false,
        })
        .unwrap();
    let operation = bind(&custom_declaration(), graph(), registry, owner.clone()).unwrap();
    call(&operation, &candidate()).unwrap();
    assert_eq!(owner.snapshot(), candidate());
}

#[test]
fn project_gate_contract_and_graph_filtering_cannot_bypass_validation() {
    for (declared_output, effectful) in [(ValueType::Object, false), (ValueType::Boolean, true)] {
        let owner = Arc::new(SnapshotOwner::new(initial()));
        let mut registry = registry();
        registry
            .register(ProjectGate {
                result: GateResult::Allow,
                declared_output,
                effectful,
            })
            .unwrap();
        let error = bind(&custom_declaration(), graph(), registry, owner.clone())
            .err()
            .unwrap();
        assert_eq!(error.code, "ONTOLOGY_DECLARATION_INVALID");
        assert_eq!(owner.commits.load(Ordering::SeqCst), 0);
    }
    let mut filtered = graph();
    filtered.nodes[1]
        .input_selector
        .include
        .insert("title".into());
    let owner = Arc::new(SnapshotOwner::new(initial()));
    let error = bind(&ontology(), filtered, registry(), owner)
        .err()
        .unwrap();
    assert_eq!(error.code, "ONTOLOGY_GRAPH_BYPASS");
}

#[test]
fn forbidden_relation_removal_and_duplicate_identity_are_rejected() {
    let mut current = candidate();
    current.objects[0]["state"] = json!("draft");
    let owner = Arc::new(SnapshotOwner::new(current.clone()));
    let operation = bind(&ontology(), graph(), registry(), owner.clone()).unwrap();
    let mut removed = candidate();
    removed.relations.clear();
    let error = call(&operation, &removed).unwrap_err().to_string();
    assert!(error.contains("ONTOLOGY_RELATION_REJECTED"), "{error}");
    assert_eq!(owner.snapshot(), current);
    let mut duplicate = candidate();
    duplicate.objects.push(duplicate.objects[0].clone());
    let error = call(&operation, &duplicate).unwrap_err().to_string();
    assert!(error.contains("ONTOLOGY_OBJECT_REJECTED"), "{error}");
    assert_eq!(owner.snapshot(), current);
}

#[test]
fn legal_identity_components_do_not_collide_at_separator_boundaries() {
    let mut candidate = candidate();
    let mut group = candidate.objects[1].clone();
    group["scope"] = json!("demo\u{1f}extra");
    group["id"] = json!("group");
    candidate.objects.push(group.clone());
    group["scope"] = json!("demo");
    group["id"] = json!("extra\u{1f}group");
    candidate.objects.push(group);
    let mut current = candidate.clone();
    current.objects[0]["state"] = json!("draft");
    current.relations.clear();
    let owner = Arc::new(SnapshotOwner::new(current));
    let operation = bind(&ontology(), graph(), registry(), owner.clone()).unwrap();
    call(&operation, &candidate).unwrap();
    assert_eq!(owner.snapshot(), candidate);
}

#[test]
fn multiple_relation_replacements_pair_each_target_once() {
    let mut declaration = ontology();
    declaration["execution"]["relations"][0]["forward"]["max"] = Value::Null;
    declaration["execution"]["relations"][0]["replace_by"] = json!(["activate"]);
    declaration["execution"]["relations"][0]["create_by"] = json!([]);
    let mut current = initial();
    for id in ["g2", "g3", "g4"] {
        let mut group = current.objects[1].clone();
        group["id"] = json!(id);
        current.objects.push(group);
    }
    let relation = candidate().relations.remove(0);
    for id in ["g1", "g2"] {
        let mut relation = relation.clone();
        relation["to"]["id"] = json!(id);
        current.relations.push(relation);
    }
    let mut candidate = current.clone();
    candidate.objects[0]["state"] = json!("active");
    candidate.relations[0]["to"]["id"] = json!("g3");
    candidate.relations[1]["to"]["id"] = json!("g4");
    let owner = Arc::new(SnapshotOwner::new(current));
    let operation = bind(&declaration, graph(), registry(), owner.clone()).unwrap();
    call(&operation, &candidate).unwrap();
    assert_eq!(owner.snapshot(), candidate);
}

#[test]
fn relation_scope_uses_the_actual_existing_endpoints() {
    let mut candidate = candidate();
    candidate.objects[1]["scope"] = json!("other");
    candidate.relations[0]["to"]["scope"] = json!("other");
    let mut current = initial();
    current.objects[1]["scope"] = json!("other");
    let owner = Arc::new(SnapshotOwner::new(current.clone()));
    let operation = bind(&ontology(), graph(), registry(), owner.clone()).unwrap();
    let error = call(&operation, &candidate).unwrap_err().to_string();
    assert!(error.contains("crosses scope"), "{error}");
    assert_eq!(owner.snapshot(), current);

    let mut declaration = ontology();
    declaration["execution"]["relations"][0]["scope"] = json!("cross_scope");
    let operation = bind(&declaration, graph(), registry(), owner.clone()).unwrap();
    call(&operation, &candidate).unwrap();
    assert_eq!(owner.snapshot(), candidate);
}

#[test]
fn candidate_object_addition_and_removal_are_rejected_without_commit() {
    let mut added = candidate();
    let mut extra = added.objects[1].clone();
    extra["id"] = json!("extra-group");
    added.objects.push(extra);
    let mut removed = candidate();
    removed.objects.remove(1);
    removed.relations.clear();

    for candidate in [added, removed] {
        let owner = Arc::new(SnapshotOwner::new(initial()));
        let operation = bind(&ontology(), graph(), registry(), owner.clone()).unwrap();
        let error = call(&operation, &candidate).unwrap_err().to_string();
        assert!(error.contains("ONTOLOGY_OBJECT_REJECTED"), "{error}");
        assert_eq!(owner.snapshot(), initial());
        assert_eq!(owner.commits.load(Ordering::SeqCst), 0);
    }
}
