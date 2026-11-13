# AppSDK Ontology Governance

## 1. Purpose

This document is the AppSDK source-repository design for ontology-based rule
governance. It defines how user intent and project memory become an approved
semantic model, then an executable DAG edge or gate.

Ontology is not a separate business model. It is the layer that makes a rule's
scenario, trigger, allowed and forbidden action, and affected state explicit
before it becomes a machine check.

## 2. Governance order

```text
affected rule / memory inventory
  -> separate rule candidates from non-rule facts
  -> Object / Behavior / Relation model
  -> per-clause user approval
  -> governance DAG
  -> executable edge / gate with legal and illegal cases
  -> red test
  -> unique-owner implementation
  -> real public-entrypoint acceptance
```

This order is mandatory for approved ontology rules. A documentation edit
alone is not enforcement. A requirement, plan, note, or memory record is not a
gate.

## 3. Three-axis model

- **Object** defines what a thing is, its identity, attributes, observable
  entrypoint, required and forbidden attributes.
- **Behavior** defines what a thing does: precondition, input, output,
  transition, effects, and the observable event or receipt.
- **Relation** defines how objects relate: direction, cardinality, observable
  entrypoint, required and forbidden relations.

The executable guidance manifest carries these axes as `ontology.objects`,
`ontology.behaviors`, and `ontology.relations`. Each rule clause names a
matching `axis_id`. A relation must resolve both endpoints to declared object
ids, and one entry's `must_have` and `must_not_have` constraints must not
contradict.

The complete reusable template, including the rule clause fields, candidate
JSON shape, and gate kinds, is
[appsdk-project-governance ontology-governance.md](../../sdk-skill-sources/appsdk-project-governance/references/ontology-governance.md).

## 4. Mandatory rule clause fields

Every approved rule clause records:

```text
scenario
trigger
allowed_actions
forbidden_actions
state_before
state_after
effects
owner
approval
evidence
governance edge / gate
```

A clause without one of those fields is a candidate. It cannot be treated as
a confirmed rule.

## 5. Enforcement boundary

- Approved ontology rules are mandatory inside their declared scope.
- Unapproved or unbound clauses remain candidates and cannot drive
  implementation, debug conclusions, or acceptance.
- The requirements ledger is a declaration of user requirements. It has no
  enforcement power.
- The enforcement truth is an executable edge or gate plus its legal case,
  illegal case, and real observation path.
- A guidance rule that claims ontology governance references an
  `ontology_clause`. `appsdk guide compile` validates the three-axis model
  entry, the clause `axis_id`, the strict bidirectional one-to-one rule-clause
  pair, the approval source, the approval text, the legal case, the illegal
  case, and the observation path before writing the compiled manifest.
  Candidate, unbound, duplicate, or mismatched clauses are rejected.
- Non-rule project facts do not enter approval. They are recorded directly with
  their evidence at their normal owner.
- If any rule or ontology candidate is waiting for user approval, do not arm,
  renew, or subscribe the next wake for that goal.

## 6. Rule ownership

| Content | Owner |
| --- | --- |
| project facts and boundaries | project `AGENTS.md` |
| reusable procedure and template | AppSDK governance Skill |
| enforceable machine rule | DAG edge, gate, or declared contract |
| user requirement declaration | requirements ledger |
| historical conclusion | memory |

The same rule is not maintained twice. A project may cite a Skill template, but
an executable gate must have one owner and one observation path.

## 7. DAG relation

Ontology is the semantic input. DAG is the executable topology. The DAG must
not invent behavior that has no approved ontology clause. An approved ontology
clause that is not bound to a DAG edge or gate is still a candidate, because
there is no executable check.

If object behavior, relation, or state truth changes, update the ontology,
then the affected gate or DAG edge, then the unique implementation. The DAG
does not rewrite ontology truth to match current code.

## 8. Execution declaration

An ontology that must be executed carries `ontology.execution` in the Guidance
manifest. The optional extension preserves the existing meaning of advisory
and legacy manifests that omit `execution`.

The extension declares:

- object states and the object-contract operator
- relation scope, cardinality, and the behaviors allowed to create, replace,
  or remove it
- one operation for each `(object, from-state, event)` dispatch
- the operation graph, protected commit node, and required clauses.

The object-contract and project gates are registered operators. Built-in
gates are `ontology.object@1`, `ontology.behavior@1`, `ontology.relation@1`,
and the alias `ontology.transition@1`. A project gate must accept an
`Object`, return `Bool`, and declare no effects. The protected
`governance.commit@1` operator is reserved by the SDK and must not be
registered by the project.

`pipeline_runtime::governance::validate_execution_declaration` performs the
declaration-only checks: required fields, unique identities, relation
endpoints, approved clauses, matching axes, graph/commit-edge references, and
duplicate object/state/event dispatch. Project function resolution and graph
binding occur in the consumer process.

## 9. Consumer binding and execution

A Rust consumer obtains the graph JSON, constructs its existing `Registry`,
and passes its commit owner to:

```rust
pipeline_runtime::governance::bind_operation(
    ontology,
    behavior_id,
    graph,
    registry,
    owner,
    capabilities,
)
```

`bind_operation` resolves the operation and required operators through the
existing `Registry`. It also rejects:

- a graph that does not match the declared graph ID;
- a missing or non-protected commit node;
- effectful nodes outside the protected commit;
- filtered or item-iterated nodes on the governed path;
- a commit node that does not dominate the single input-to-output path;
- a clause whose governance edge is not a real incoming edge to the commit;
- an unresolved project operator or gate.

The returned `CompiledOperation` is the single production entry. Its `call`
method accepts an `OperationRequest`, DAGPipe `Identity`, and `Cancellation`.
The SDK replaces the protected commit template with a per-call
`InvocationCommitOperator`. That operator calls the consumer's `CommitOwner`.

The consumer owner is responsible for its storage consistency boundary. It
constructs the complete candidate `GovernanceSnapshot`, calls the provided
validate closure, and commits only after validation succeeds. The closure
checks object identity and contracts, relation endpoints, scope, cardinality
and lifecycle permissions, current state, target state, and required clauses.
Current and candidate snapshots carry the same object identities in this
increment. Each operation changes the declared state, attributes and relations
of those existing objects.
The SDK does not add a transaction engine, retry, or automatic rollback.

The call returns `ExecutionResult`, an admission `GovernanceError`, or the
normal DAGPipe `ExecutionFailure`. Error codes distinguish declaration,
reference, gate, graph bypass, object, relation, transition, conflict, and
owner failures. A rejected admission occurs before the owner commits the
candidate.

## 10. Consumer observation

The isolated `governed_catalog` example is the first black-box consumer:

```text
catalog.prepare
  -> governance.commit
  -> catalog.receipt
```

The example reads a static ontology declaration and graph, binds the
operation once, and executes a JSON script. `apply` submits through
`CompiledOperation::call`; `query` reads the consumer-owned state. Rejected
requests return an explicit JSON error and leave the record and relation
state unchanged.

This example proves the mechanism for one isolated consumer. It does not
approve a production business clause or claim that Collab, Memory, or another
production project has been migrated.

## 11. Architecture changes during a project

The consumer updates its ontology and operation graph, checks the affected
clauses and operators, and binds a replacement `CompiledOperation` for future
calls. Each bound operation owns its compiled graph and immutable contract
bindings. Calls already using the previous operation keep those bindings
until they finish.

The project owner coordinates changes to persisted state and storage adapters
inside its existing consistency boundary. It supplies the current state to
the validation callback at commit time. A call whose source state has become
invalid receives the normal transition rejection. This keeps the governance
mechanism usable while the project evolves its architecture.
