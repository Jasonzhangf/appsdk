# AppSDK Development Process Control Harness

## L0 Purpose

The Development Process Control Harness turns AppSDK governance into a
lightweight, stateful process for an agent. It compiles declared project rules,
persists an agent-authored plan, accepts evidence-bearing step results,
projects current state, and returns the next legal step.

The harness helps work converge. It does not become a second lifecycle, a
second agent runtime, or a global prerequisite for ordinary `verify` and
`compile`.

## L1 Boundaries

```text
AGENTS.md
  project facts, architecture, owners, hard boundaries

SKILL.md
  agent trigger, operating loop, reporting contract

appsdk-guidance.json
  machine nodes, edges, evidence requirements, severity

Process Control Harness
  deterministic compile, intake, validation, projection, persistence

Agent
  authors the plan, executes one step, returns observations/evidence

Project adapters
  produce real build/install/restart/runtime evidence
```

AppSDK never calls a model or provider. The agent already supplies the model
runtime. Guidance returns a bounded context and schema; the agent proposes the
technical plan.

## L2 Truth and storage

Existing AppSDK project, module, artifact, review, promotion, and freeze records
remain the sole lifecycle truth. The harness projects those records and must
not invent a parallel lifecycle enum.

```text
.appsdk/guidance/compiled.json
  committed deterministic rule-context manifest

.appsdk-control/guidance/<task-id>/
  ignored active PlanRecord and append-only execution/revision events

.appsdk/records/guidance/<task-id>/
  optional immutable lifecycle checkpoints owned by later milestones
```

`next_transition` is always derived from the compiled manifest, current plan,
events, rule-context snapshot, and existing lifecycle state. It is never an
agent-writable truth field.

`GuidanceSetupProposal` and `PlanProposal` are separate. The former is a
project-level, user-approved proposal for durable AGENTS, Skill,
machine-contract, and source-declaration changes. The latter is task-local
execution state and never becomes a durable project rule automatically.

## L3 Functional separation

| Function | Input | Output | Does not own |
|---|---|---|---|
| Rule compiler | declared AGENTS, Skill, JSON contracts | deterministic rule context | prose semantics |
| Plan controller | agent PlanProposal | validated PlanRecord/revision | technical decisions |
| Execution ledger | step result + evidence refs | append-only events | evidence production |
| State projector | lifecycle + declared rules + plan + events | intake questions, readiness, blocker, next node | user answers or lifecycle mutation |
| Lifecycle bridge | domain/node mapping | canonical command/gate hints | duplicate commands |
| Closeout projector | final state/evidence | gaps, cleanup, memory candidates | automatic memory writes |

## L4 Full-domain routing

One engine covers AppSDK as a whole. Existing governance without Guide enters
through a read-only setup proposal before rule compilation. Configured projects
may re-enter the same bootstrap path for a non-destructive standard-template
upgrade review:

```text
guide status -> bootstrap setup proposal -> user approval -> guide compile
             -> bootstrap -> migration -> governance-preflight
             -> develop/debug -> review -> delivery
             -> integration -> promotion -> freeze -> cleanup
```

Domains select prompts and workflow contracts. They do not duplicate canonical
commands such as `prepare`, `init`, `pin-lock`, `verify`, `compile`,
`verify --review-admission`, `promote`, `publish-active`, or `freeze`.

`guide init` is the read-only intake before a setup proposal or domain plan.
Bootstrap mode always discovers a bounded set of current project-document
candidates plus the installed versioned standard template, projects existing
project/module state, and asks the Agent to present a `GuidanceSetupProposal`
for user approval. With compiled Guidance it returns a
`template_upgrade_review`; otherwise it returns an initial setup. It does not
write project or task state, and the template is not activated as a rule
source. After approval and compile, task intake
projects only explicitly declared AGENTS and local Skill sources, their
precedence/digests, project/module/goal context, unresolved questions, Skill
invocation suggestions, and next commands. AppSDK does not interpret prose,
answer questions, call a model, or persist a second intake truth.

## L5 Rule context

The project explicitly declares every compiled rule source in
`.appsdk/project.json`. Runtime directory scanning is forbidden. The only
pre-declaration discovery is bootstrap intake over root `AGENTS.md`, the
installed standard template reference, the bundled AppSDK Skill, and one direct
Skill child under each standard project-local Skill root. Discovery produces
candidates, not active rules. The standard template remains advisory and cannot
be declared implicitly. User approval and an explicit project declaration are
required before compile. Markdown sources are
then bound by path, precedence, and digest for the agent to read; AppSDK does
not claim semantic verification of arbitrary prose. Machine contracts are JSON
and are validated.

A PlanRecord binds:

- project, goal, module, task, owner, and mode;
- project contract, goal, source commit/tree, and scope hashes;
- declared AGENTS/Skill source paths and digests;
- compiled guidance manifest hash.

Any bound-context drift blocks updates until a PlanRevisionRecord is accepted.

## L6 Severity

The harness uses three severities:

- `advisory`: recommendation only;
- `warning`: visible debt; work may continue;
- `forbidden`: unsafe or dishonest transition; command fails.

Default control is advisory. Projects may explicitly raise enforcement for a
workflow. Missing PlanRecord never makes ordinary AppSDK `verify` or `compile`
fail.

Default forbidden boundaries are narrow:

- fabricated or empty evidence for a passing required step;
- non-adjacent transitions;
- overwrite/delete of plan history;
- stale commit/tree/scope/rule-context evidence;
- continuing an old plan after owner, scope, or rule-contract drift;
- guidance mutation from `main`;
- claiming review, delivery, promotion, freeze, or cleanup complete before the
  corresponding required lifecycle gate passes.

Architecture rules are enforced against the candidate change set. New or
modified behavior must preserve typed control truth, one semantic owner and
implementation, a fixed lifecycle skeleton, configured operations, registered
hooks, declared gates, ablation before addition, and shared-function reuse.
Untouched historical violations are advisory findings unless they enter changed
scope or affect safety, ownership, evidence truth, or required delivery.

Before compiling or projecting a plan, the state projector checks that the
selected module's declared source and contract surfaces exist. A missing path
is a project module-binding problem: status names the module, surface, path,
canonical owner, and project-contract repair action. It is never reported as a
missing global package, communication failure, or external SDK-owner wait.

## L7 Idempotency and failure

Every step update has an `event_id`.

- same ID + same canonical content: successful no-op;
- same ID + different content: `GUIDANCE_EVENT_CONFLICT`;
- failure: append the evidence-bearing event and project `blocked`;
- retry: only after a new plan revision or an unchanged idempotent replay.

Errors expose the first failing rule, current lifecycle projection, preserved
state, retry permission, and one legal next action. No polling or automatic
retry is part of guidance.

## L8 Close boundary

`guide close` reports workflow completion, remaining lifecycle gaps, evidence,
cleanup requirements, and memory candidates. The first release never writes
AGENTS, Skills, MEMORY, or USER files automatically. Durable-rule changes use a
separate human-reviewed change.

## L9 Human tour and process review

`guide tour` lets a human inspect the generated workflow and persist an explicit
node path. `guide review` is staged over the same append-only task ledger:
`node_review` checks and accepts node-content revisions first; only after every
selected node has an accepted revision can `flow_review` update edges, order, or
rules. Flow patches retain the accepted node revision IDs and remain staged;
they do not silently mutate the active compiled manifest. The memory reminder
on review nodes is advisory and invokes the independent `project-memory review`
command only at task close.
