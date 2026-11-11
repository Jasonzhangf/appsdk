# Ontology Rule Governance

This reference is the reusable template for AppSDK rule upgrades. It sits
between user intent or project memory and the executable governance graph.

## 1. Ownership and boundary

Ontology models business and control semantics. DAG nodes and edges execute
those semantics. A gate rejects a concrete invalid transition or effect.

- Project `AGENTS.md`, project Skills, and declared contracts own the
  effective rule text.
- This Skill owns the rule-upgrade procedure and ontology template.
- Existing memory is an input to renovation, never proof that a rule is
  approved or enforceable.
- `.appsdk/requirements.json` records user requirements. It is descriptive.
  It does not enforce behavior.
- Only an executable edge/gate can make an approved rule mandatory.
- A clause without an approved owner and executable gate remains `candidate`.

Do not put unrelated project facts into rule approval. Record facts with their
evidence at their normal project owner.

## 2. Three-axis model

Every rule candidate describes all three axes that affect the change.

### Object: what it is

Record:

```text
object_id
name
definition
identity
owner
attributes
perception
must_have
must_not_have
source
approval
governance
```

`identity` states when two instances are the same business object.
`perception` names the public entrypoint, query, event, or receipt that makes
the object observable. Attribute constraints must be observable.

### Behavior: what it does

Record:

```text
behavior_id
name
owner
precondition
input
output
transition { from, to }
effects
perception
must_have
must_not_have
source
approval
governance
```

Write the state change as:

```text
(current_state, input) -> (output, next_state, effects)
```

Keep request acceptance, action execution, and business completion separate.
Failure, rejection, cancellation, and unknown result are distinct terminals.

### Relation: how objects relate

Record:

```text
relation_id
name
from
to
kind
direction
cardinality
perception
must_have
must_not_have
source
approval
governance
```

State the direction and cardinality. For example, one durable message may
have exactly one current recipient. A relation without a checkable observation
path is not enforceable.

## 3. Rule clause format

Every rule clause answers: what scenario triggers it, what action it allows or
forbids, and what state or effect changes.

```text
clause_id:
  axis: object | behavior | relation
  scope:
  rule:
  scenario:
  trigger:
  allowed_actions:
  forbidden_actions:
  state_before:
  state_after:
  effects:
  owner:
  approval: candidate | approved | rejected
  evidence:
  governance:
    edge:
    gate_kind:
    check:
    allow_case:
    deny_case:
    observation:
```

The clause is incomplete when it lacks a scenario, trigger, effect or state
impact, owner, evidence path, or gate binding.

## 4. Governance flow

Use this order:

1. Inventory the affected existing rules, project facts, code paths, DAGs, and
   memories that need renovation.
2. Separate rule candidates from non-rule facts. Facts do not enter approval.
3. Model affected Objects, Behaviors, and Relations.
4. Ask the user to approve or revise each rule clause. Do not treat approval of
   the overall model as approval of its individual clauses.
5. Generate or update the governance DAG from approved clauses.
6. Bind each approved clause to one or more executable edges/gates.
7. Add legal and illegal examples. Run the red test before implementation.
8. Modify only the unique owner's code.
9. Re-run the real public entrypoint and record the result.

While any rule or ontology candidate awaits user approval, do not arm, renew,
or subscribe the next wake. A wake cannot make an unapproved clause
enforceable.

Legacy memory that predates object/behavior/relation modeling has no clause
scenario, trigger, allowed or forbidden actions, state effect, owner, evidence,
or approval state. Before such memory is used as rule input: renovate it into
candidate rule clauses with the three-axis fields, or record it as a non-rule
project fact with its evidence. Stale memory never approves a rule by itself,
never triggers a wake, and never acts as an enforceable gate.

## 5. Gate kinds

| Gate | Rejects |
| --- | --- |
| `identity` | missing, ambiguous, or conflicting object identity |
| `attribute` | missing required attribute or forbidden attribute |
| `precondition` | action starts from an invalid state |
| `transition` | forbidden state change |
| `relation` | missing required relation, forbidden relation, or bad cardinality |
| `terminal` | unclosed, synthetic, or mixed success/failure ending |
| `effect` | forbidden side effect |

Each gate needs one legal case, one illegal case, and a real observation path.

## 6. Executable guidance gate

When an ontology-managed rule is distributed through an AppSDK guidance
manifest, the rule carries an `ontology_clause` reference. The manifest's
`ontology.clauses` entry records the complete clause, including its approval
source and text, and its governance edge or gate.

`appsdk guide compile` is the executable owner for that declaration. It
rejects a referenced clause when approval is not `approved`, when approval
source or text is missing, when a governance field is missing, or when the
rule reference does not resolve. It compiles an approved and fully bound
clause. The public observation is the command's JSON error or successful
compiled manifest.

The manifest schema is
[`contracts/guidance/guidance-manifest.schema.json`](../../../contracts/guidance/guidance-manifest.schema.json).
Do not describe a manifest-only clause as mandatory unless this compiler gate
is present.

## 7. Rule ownership

Update only the owner of the rule:

| Rule content | Owner |
| --- | --- |
| project facts and boundaries | project `AGENTS.md` |
| reusable procedure | project or AppSDK Skill |
| enforceable machine rule | declared contract, DAG edge, or gate |
| user requirement declaration | requirements ledger |
| historical conclusion and retrieval summary | memory |

Do not claim that a Skill, AGENTS edit, or requirements entry enforced a rule.
The enforcement receipt comes from the bound gate and its real observation.

## 8. Candidate encoding

The optional JSON shape below is a modeling aid. The project may keep the same
fields in Markdown until machine use exists.

```json
{
  "ontology_version": 1,
  "objects": [
    {
      "id": "",
      "name": "",
      "definition": "",
      "identity": "",
      "owner": "",
      "attributes": [],
      "perception": "",
      "must_have": [],
      "must_not_have": [],
      "source": [],
      "approval": "candidate",
      "governance": []
    }
  ],
  "behaviors": [
    {
      "id": "",
      "name": "",
      "owner": "",
      "precondition": [],
      "input": [],
      "output": [],
      "transition": {"from": "", "to": ""},
      "effects": [],
      "perception": "",
      "must_have": [],
      "must_not_have": [],
      "source": [],
      "approval": "candidate",
      "governance": []
    }
  ],
  "relations": [
    {
      "id": "",
      "name": "",
      "from": "",
      "to": "",
      "kind": "",
      "direction": "",
      "cardinality": "",
      "perception": "",
      "must_have": [],
      "must_not_have": [],
      "source": [],
      "approval": "candidate",
      "governance": []
    }
  ],
  "clauses": []
}
```

## 9. DAG boundary

The ontology is the semantic input. The DAG is the executable topology. DAG
edges and gates must not invent behavior that has no approved ontology clause.

When behavior, state, or relation truth changes, update the ontology first,
then the affected DAG edge/gate, then the unique implementation.

The AppSDK source design is `docs/design/appsdk-ontology-governance.md` in the
AppSDK repository.
