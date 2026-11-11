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
  `ontology_clause`. `appsdk guide compile` rejects candidate or unbound
  clauses and validates the approval source, approval text, legal case,
  illegal case, and observation path before writing the compiled manifest.
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
