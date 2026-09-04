# Development Process Control Harness

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

## Start

```bash
appsdk guide status <project> --task <task-id>
appsdk guide compile <project>
appsdk guide develop <project> --module <module-id>
```

Use another domain when appropriate. Read only returned declared rule sources.

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
appsdk guide plan <project> --task <task-id> --input plan.json
```

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
appsdk guide update <project> --task <task-id> --input result.json
appsdk guide next <project> --task <task-id>
```

Same event ID and content is idempotent. Same ID with different content fails.
`pass` with required but absent evidence fails. Only projected step may update.

## Revision

Resubmit PlanProposal with `revision_reason` when evidence, blocker, hypothesis,
scope, owner, source, environment, rules, contracts, gates, or dependencies
change. Old plan/events remain append-only. Never edit control files by hand.

## Close

```bash
appsdk guide close <project> --task <task-id>
```

Read `workflow_complete` and `appsdk_lifecycle_complete` separately. Apply
memory candidates only through a separate reviewed change.
