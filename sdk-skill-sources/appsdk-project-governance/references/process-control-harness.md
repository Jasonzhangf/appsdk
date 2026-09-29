# Development Process Control Harness

This reference applies only when persistent Guidance is selected. `advisory`
and `warning` never require setup, a plan or workflow close for independent
development. Quality admission remains in canonical AppSDK gates. A missing
optional setup returns `guide_flow_required: false`; do not start a setup detour.

## Functions

```text
rule compiler      declared sources -> deterministic rule context
plan controller    PlanProposal -> PlanRecord/PlanRevisionRecord
execution ledger   step result -> append-only StepExecutionRecord
state projector    lifecycle + plan + events -> readiness/blocker/next
lifecycle bridge   domain node -> canonical AppSDK command/gate
closeout projector final state -> gaps/cleanup/memory candidates
```

Harness does not call a model, produce project evidence, mutate lifecycle truth,
or write memory automatically.

## Tour and ordered review

Use `appsdk guide tour --task <id> --mode <domain>` to inspect the generated
workflow and let a human choose an adjacent path. Persist that choice with a
TourProposal input when needed. Submit `appsdk guide review` in two stages:
`node_review` checks and accepts node content first; only after every selected
node has an accepted revision may `flow_review` update edges, order, or rules.
The accepted flow patch retains node revision IDs and stays staged until the
declared source is explicitly updated and compiled.

## Start

```bash
appsdk guide status --task <task-id>
appsdk guide init --task <task-id> --mode develop --module <module-id>
appsdk guide develop --task <task-id> --module <module-id>
```

If status returns `GUIDANCE_SETUP_REQUIRED`, do not call compile yet:

```bash
appsdk guide init --task guidance-setup --mode bootstrap --module <module-id>
```

Read the returned candidate sources, produce the requested
`GuidanceSetupProposal`, and obtain explicit user approval. The Agent then
updates project-owned human and machine rule sources in a clean owner worktree,
declares them in `.appsdk/project.json`, and runs `appsdk guide compile` plus
`appsdk verify`. If status is only `GUIDANCE_NOT_COMPILED`, approved sources are
already declared and compile is the next command.

After an AppSDK update, or for an explicit rules refresh, a configured project
uses the same read-only bootstrap intake:

```bash
appsdk init
appsdk guide init --task guidance-upgrade --mode bootstrap --module <module-id>
```

Read current project sources before the returned standard template reference.
The resulting `template_upgrade_review` proposal may recommend changes, retain
project rules, or decline template items. It does not write state or activate
the template. Apply only user-approved differences in a clean owner worktree,
then compile and verify.

Use `--mode debug` for a bug, regression, or incident. Use another declared
domain when appropriate. `guide init` is read-only and returns:

- declared AGENTS and local Skill paths to read in precedence order;
- unresolved develop/debug questions to ask the user;
- exact `$skill-id` suggestions for declared local Skills;
- missing commands and the next `appsdk guide` command sequence.

Ask only questions still unresolved after reading the returned sources and the
current user request. Then run the projected domain command and write the
PlanProposal. `appsdk guide --help` lists the full command surface.

`GuidanceSetupProposal` is project-level and user-approved. `PlanProposal` is
task-level and stored in local control state. Never promote a task plan into a
project Skill automatically.

If status returns `MODULE_PATH_MISSING`, treat it as a project module-binding
error. The named module owner updates that module's `owned_paths` or
`contract_paths` to real project paths in `.appsdk/project.json`, recompiles
guidance, and verifies. Do not classify it as a missing global package, Collab
failure, or reason to wait for an external AppSDK owner.

## PlanProposal

```json
{
  "schema_version": 1,
  "mode": "develop",
  "goal_id": "goal-id",
  "task_id": "task-id",
  "module_id": "module-id",
  "objective": "bounded objective",
  "scope_paths": ["declared/module/path/**"],
  "steps": [{
    "step_id": "step-1",
    "node_id": "requirements",
    "action": "concrete action",
    "owner": "module-id",
    "expected_evidence": ["requirements"]
  }]
}
```

Do not provide `current_node`, `next_transition`, source hashes, scope hash, or
rule-context hash. Harness derives them.

```bash
appsdk guide plan --task <task-id> --input plan.json
```

This is the first task-state write. It creates the active PlanRecord under
`.appsdk-control/guidance/<task-id>/plan.json`; initialization does not create a
second intake truth.

## Update

```json
{
  "schema_version": 1,
  "event_id": "stable-unique-id",
  "step_id": "step-1",
  "result": "pass",
  "observations": ["observed fact"],
  "evidence": ["evidence-id"]
}
```

```bash
appsdk guide update --task <task-id> --input result.json
appsdk guide next --task <task-id>
```

Same event ID and content is idempotent. Same ID with different content fails.
`pass` with required but absent evidence fails. Only projected step may update.

## Revision

Resubmit PlanProposal with `revision_reason` when evidence, blocker, hypothesis,
scope, owner, source, environment, rules, contracts, gates, or dependencies
change. Old plan/events remain append-only. Never edit control files by hand.

## Close

```bash
appsdk guide close --task <task-id>
```

Read `workflow_complete` and `appsdk_lifecycle_complete` separately. Apply
memory candidates only through a separate reviewed change.
