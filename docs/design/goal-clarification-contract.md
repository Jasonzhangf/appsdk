# Goal Clarification Contract

This contract applies when a project has explicitly enabled the
`GoalClarificationRecord` admission workflow. Ordinary development and debug
follow the host workflow; this record is not a second universal entry gate.

```text
User request
  -> parse objective
  -> restate understanding
  -> acceptance criteria
  -> non-goals and assumptions
  -> ambiguities and focused questions
  -> reuse existing explicit authorization or resolve material unanswered questions
  -> claim/admission
  -> Playground or implementation
```

`GoalClarificationRecord` stores the raw request, understood objective, acceptance criteria, non-goals, assumptions, ambiguities, questions/answers, scope, risk decisions, confirmation identity/time, and lifecycle status.

`confirmed` means the goal and scope are explicitly authorized and all material
questions are answered. Existing user instructions can supply that authorization;
do not require another restatement/confirmation round for the same accepted goal.
`admitted` additionally means feature/module owner, allowed paths, and required
gates are bound. Record the real authorization, not an invented approval.

For bugs, this contract defines expected behavior and non-goals first; evidence-first debugging still requires baseline reproduction, first divergence, positive/negative intervention, and unique-owner evidence in Playground.

Within this opted-in admission workflow, unresolved material questions stop only
their dependent claim or mutation. Continue independent authorized work. A
pending record does not add a project-wide stop or require routine promotion or
freeze. Record state alone does not replace behavior evidence or host plan/review.

## Formal project-goal change

`appsdk goal [project] apply --input <request.json> [--json]` is the formal write
entry for `.appsdk/goal.json`. It creates the first project goal or replaces the
current project goal. It is separate from `goal subscribe`, `goal edit`,
`goal update`, and `goal cancel`, which manage long-horizon subscriptions in
`.appsdk-control`.

The request contains `operation`, `project_id`, `base_goal_revision`,
`requirements_version`, `authorization`, and a complete `goal`. `operation` is
`create` or `replace`. A create requires no current goal and a null base. A
replace requires the current goal and an exact base revision. The request
requirements version is explicit and must equal the ledger total version read
inside the project transaction lock. The CLI never silently repairs it.

The mutation path reads the old goal without requiring its old requirements
binding to remain current. This allows a formally authorized replacement to
recover a stale goal. Ordinary consumers keep the existing stale rejection.
The writer validates old and new structure, preserves the old goal in a flat
server-managed replacement history, and atomically writes one complete goal
record. Rejected or failed requests do not change the goal or unrelated project
data.

`clarification_pending` may be recorded. `confirmed` requires complete
confirmation fields, no open questions, and a traceable user authorization.
Callers cannot set `admitted`; the existing claim, owner, and required-gate path
still owns that transition.

### Ontology

Object:

- `ProjectGoal`: the project's zero-or-one current goal record. Identity is
  `project_id + goal_id + goal_revision`. Owner is `registry.rs`. It must carry
  real goal content and exactly one requirements binding. It must not carry a
  long-horizon subscription as its binding.
- `GoalChangeRequest`: one public `goal apply` input. Identity is the request's
  operation, project, and base revision. It must not choose the output path or
  claim server-owned history.
- `GoalReplacementHistory`: server-managed flat entries in the current goal.
  Each entry preserves the previous goal value and records the superseding
  revision. Callers cannot rewrite history.
- `RequirementsLedger`: the existing project requirement total version. Its
  owner remains `requirements.rs`, and goal change must not modify it.

Behavior:

- `(no goal, valid create)` -> `(applied, revision 1, current goal written)`.
- `(current goal, valid replace and matching base)` ->
  `(applied, revision + 1, previous goal preserved in history)`.
- `(any state, invalid content or authorization)` ->
  `(rejected, no mutation)`.
- `(changed base or busy lock)` -> `(conflict or busy, no overwrite)`.
- `(write failure)` -> `(failed, old goal preserved)`.

Relation:

- A project has zero or one current goal.
- A current goal binds exactly one current requirements total version.
- One successful replace relates one previous goal value to one superseding
  revision.
- Project goals and long-horizon subscriptions have no automatic relation.

These requirements come directly from the accepted project-goal capability
request and the existing goal clarification contract. Their executable gates
are the public `goal apply` checks and the unchanged goal consumers.
