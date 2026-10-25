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

## Optional tour and workflow editing

Use `appsdk guide tour --task <id> --mode <domain>` when explicitly inspecting
or editing the generated workflow. It is not a required human decision before
ordinary task execution. Persist an authorized choice with a TourProposal
when needed. Workflow editing uses `appsdk guide review` in two stages:
`node_review` checks and accepts node content first; only after every selected
node has an accepted revision may `flow_review` update edges, order, or rules.
The accepted flow patch retains node revision IDs and stays staged until the
declared source is explicitly updated and compiled.

## Start

```bash
appsdk guide init --task <task-id> --mode develop --module <module-id>
```

Use an existing valid status/intake result directly. Query status only for a
specific missing state. If it returns `GUIDANCE_SETUP_REQUIRED`, do not call compile yet:

```bash
appsdk guide init --task guidance-setup --mode bootstrap --module <module-id>
```

Read the returned candidate sources, produce the requested
`GuidanceSetupProposal`, reuse session authorization that already covers a
difference, and obtain explicit approval only for uncovered changes. The Agent
then updates project-owned human and machine rule sources in a clean owner
worktree, declares them in `.appsdk/project.json`, and runs `appsdk guide
compile` plus `appsdk verify`. If status is only `GUIDANCE_NOT_COMPILED`,
approved sources are already declared and compile is the next command.

For an intentional rule refresh, a configured project may use the same
read-only bootstrap intake. Refresh installed project resources only when
needed; an unrelated SDK update does not require this sequence:

```bash
appsdk init
appsdk guide init --task guidance-upgrade --mode bootstrap --module <module-id>
```

Read current project sources before the returned standard template reference.
Read actual test commands and CI/hook entrypoints as well as the rules. The
resulting `template_upgrade_review` proposal may recommend changes, retain
project rules, or decline template items. Record owner, action, basis, retained
safeguard, and entrypoint impact for each difference; reuse session
authorization that already covers a difference and ask only for uncovered
changes. It does not write state or activate the template. Guidance is optional:
without it, apply the same authorized audit and CI/hook changes without
compile. Repeated init or unrelated version changes do not trigger a
whole-project audit. Apply authorized differences in a clean owner worktree,
then compile and verify when Guidance is selected.

Use `--mode debug` for a bug, regression, or incident. Use another declared
domain when appropriate. `guide init` is read-only and returns:

- declared AGENTS and local Skill paths to read in precedence order;
- unresolved develop/debug questions to resolve from requirements, evidence
  and the responsible collaborator;
- exact `$skill-id` suggestions for declared local Skills;
- missing commands and the next `appsdk guide` command sequence.

Use the returned sources and current authorization first. For registered
Collab work, route remaining questions through its clarification rule: peer
facts to the owner, design to the designer, scope/resources to the master.
Ask the user only for a user-held critical fact, new authorization or a change
to an explicit user constraint. Continue independent work. Run another domain
projection only when its missing context is needed, then submit the PlanProposal.
`appsdk guide --help` lists the full command surface.

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
rule-context hash. Harness derives the current node and next transition;
ordinary work does not create a source-identity packet.

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
```

Use the update result to continue; call `guide next` only if the next action
is not already available. Same event ID and content is idempotent. Same ID with different content fails.
`pass` with required but absent evidence fails. Only projected step may update.

## Revision

Resubmit PlanProposal with `revision_reason` when goal, scope, acceptance,
key approach, declared owner or relevant dependencies materially change.
Ordinary step evidence, scoped edits and Git commits do not by themselves
require a new plan. Old plan/events remain append-only. Never edit control files by hand.

## Close

```bash
appsdk guide close --task <task-id>
```

Read `workflow_complete` and `appsdk_lifecycle_complete` separately. Apply
memory candidates only through a separate reviewed change.
