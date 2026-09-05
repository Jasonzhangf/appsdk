# AppSDK Development Process Control Harness — Detailed Design

## 1. Project contract

Projects opt in through `.appsdk/project.json`:

```json
{
  "guidance": {
    "enforcement": "advisory",
    "compiled_manifest": ".appsdk/guidance/compiled.json",
    "rule_sources": [
      {
        "source_id": "project-agents",
        "kind": "agents",
        "path": "AGENTS.md",
        "required": false,
        "precedence": 100
      },
      {
        "source_id": "appsdk-governance-skill",
        "kind": "skill",
        "path": ".appsdk/skills/appsdk-project-governance/SKILL.md",
        "contract_path": ".appsdk/skills/appsdk-project-governance/appsdk-guidance.json",
        "required": true,
        "precedence": 200
      }
    ]
  }
}
```

Paths are project-relative, normalized, non-symlinked, and explicitly listed.
Duplicate source IDs, duplicate precedence, path escape, or duplicate workflow
IDs are invalid.

## 2. Rule compiler

`appsdk guide compile [project]`:

1. reads the project contract;
2. reads only declared rule sources;
3. validates each declared JSON guidance contract;
4. orders sources by precedence then source ID;
5. rejects duplicate domain, node, edge, and rule IDs;
6. emits source digests, merged workflows/rules, and a deterministic manifest
   hash to `.appsdk/guidance/compiled.json`.

Compilation is deterministic: timestamps and absolute paths are excluded. The
same source bytes and project contract produce identical output in every
worktree.

## 3. State projector and domain prompts

Read-only commands:

```text
appsdk guide init [project] --task <id> --mode <domain> [--module <id>]
appsdk guide status [project] [--task <id>]
appsdk guide <domain> [project] [--task <id>] [--module <id>]
appsdk guide next [project] --task <id>
appsdk guide close [project] --task <id>
```

Supported domains are `bootstrap`, `migration`, `governance-preflight`,
`develop`, `debug`, `review`, `delivery`, `integration`, `promotion`, `freeze`,
and `cleanup`.

All projections include:

```json
{
  "lifecycle": {"project_stage": "...", "module_stage": "..."},
  "readiness": "ready|blocked|complete",
  "reason_code": "...",
  "first_failing_gate": null,
  "next": {"node_id": "...", "instruction": "..."}
}
```

The projector validates the selected module's declared `owned_paths` and
`contract_paths` before rule compilation. `MODULE_PATH_MISSING` returns
`module_binding` as the first failing gate and directs the module owner to bind
the project contract to an existing project-owned path. Ordinary `verify`
remains independent; compile-time source hashing retains its existing hard
failure.

`readiness` and `reason_code` explain lifecycle state; they are not lifecycle
nodes. Prompt commands add declared rule paths, allowed evidence, and the plan
proposal schema. They never choose technical implementation details.

### Initialization intake

`guide init` is the standard entry before an agent creates a task plan. It
returns `guide_flow_required`, the selected goal/module/workflow projection,
present declared sources in precedence order, missing optional context, exact
`$skill-id` suggestions from a declared Skill contract or `<skill-id>/SKILL.md`
path, and mode-specific questions. Develop intake confirms requirements,
architecture closure, and delivery depth. Debug intake confirms the failing
sample, causal experiment, and old-sample replay.

Normal task intake never scans undeclared Skill directories and never writes
task state. If approved guidance has not been compiled, it returns the missing
`guide compile` and repeatable `guide init` commands. `new` and configured
`init` print this onboarding sequence. After the Agent reads context and asks
only unresolved questions, it runs the projected domain command and submits
PlanProposal; `guide plan` is the first task-state write.

When an existing `.appsdk/project.json` has no `guidance` member, `appsdk init`
preserves that contract and all lifecycle truth, installs missing Guide bundle
resources, and prints the bootstrap intake command instead of directing the
user to compile an undeclared rule set. `guide status` returns
`GUIDANCE_SETUP_REQUIRED`.

`guide init --mode bootstrap` is a special read-only project setup and upgrade
intake. It works both before initial compile and after Guidance is configured.
It discovers only root `AGENTS.md`, the installed versioned standard template,
the bundled AppSDK Skill, and direct Skill children under `skills/`,
`.agents/skills/`, and `.codex/skills/`. Existing declared sources are included
first. Symlinked, missing, nested, and unrelated files are not ingested. Every
project source remains a candidate until user approval and explicit declaration
in the project contract. The standard template is advisory comparison material
and is never an active rule source.

Bootstrap output includes existing project/module state, candidate source paths
and digests, Skill invocation suggestions, unresolved workflow/command/rule
ownership questions, a `GuidanceSetupProposal` schema, and the post-approval
compile/verify commands. It writes neither durable rules nor
`.appsdk-control`. The Agent reads the files, reconciles project commands and
procedures, asks only unresolved questions, and presents the proposal. After
explicit approval, the Agent edits project-owned AGENTS, local Skills, machine
contracts, and the source declaration in a clean owner worktree; only then does
`guide compile` create committed rule context.

Repeated `appsdk init` refreshes `.appsdk/templates/minimal/AGENTS.md` from the
current Bundle without overwriting the project-owned root `AGENTS.md`. A
configured project may then request bootstrap with task `guidance-upgrade`.
The output uses `setup_kind=template_upgrade_review`, binds the reference path,
version, and digest, and asks the Agent to read current rules first. The
proposal separates recommended changes, retained project rules, and declined
template items. It writes no project, lifecycle, or task state before approval.
The advisory reference is outside the strict SDK resource-integrity set, so its
absence cannot block ordinary verification or unrelated delivery. `init`
restores it on demand.

`GuidanceSetupProposal` is project-level. `PlanProposal` remains task-level and
cannot automatically modify AGENTS, Skills, machine contracts, or memory.

## 4. Plan controller

The agent writes JSON and submits it with:

```text
appsdk guide plan [project] --task <id> --input <file>
```

Required proposal fields:

```json
{
  "schema_version": 1,
  "mode": "develop",
  "goal_id": "goal-1",
  "task_id": "task-1",
  "module_id": "app-core",
  "objective": "bounded objective",
  "scope_paths": ["src/**", "tests/**"],
  "steps": [
    {
      "step_id": "step-1",
      "node_id": "requirements",
      "action": "analyze requirements",
      "owner": "app-core",
      "expected_evidence": ["requirements-record"]
    }
  ]
}
```

The proposal must not contain `current_node`, `next_transition`, source hashes,
or a rule-context hash. AppSDK derives them. Nodes must form one adjacent path
from the selected domain entry. Owner must match the node's resolved canonical
owner. Scope must remain inside the module's owned/contract paths.

The persisted PlanRecord adds a RuleContextSnapshot and a canonical plan hash.
The first release stores active plans in `.appsdk-control`; it does not make an
unfinished plan part of the committed project truth.

## 5. Plan revision ledger

Submitting `guide plan` for an existing task requires `revision_reason`:

```text
new_evidence | new_blocker | hypothesis_rejected | scope_changed |
owner_changed | source_drift | environment_changed |
prior_solution_matched | rule_context_changed |
guidance_manifest_changed | project_contract_changed |
gate_contract_changed | dependency_input_changed
```

AppSDK appends a PlanRevisionRecord, retains the old PlanRecord and events, and
replaces only the active projection. Existing history is never overwritten.
Scope, owner, source, manifest, or forbidden-rule drift cannot be auto-admitted.

## 6. Execution event ledger

```text
appsdk guide update [project] --task <id> --input <file>
```

Input:

```json
{
  "schema_version": 1,
  "event_id": "event-1",
  "step_id": "step-1",
  "result": "pass",
  "observations": ["what was observed"],
  "evidence": ["evidence-id"]
}
```

Only the projected next step may be updated. `pass` on a node requiring
evidence needs at least one evidence ID. `fail` or `blocked` records evidence
and keeps the workflow blocked. Scope or owner changes require a plan revision.

Before append, AppSDK recomputes the rule context. Drift rejects the update and
returns the required revision reason. Event append is atomic and idempotent.

## 7. Lifecycle bridge

Guidance maps domains to existing lifecycle capabilities and commands. It does
not reimplement them. A step result records what the agent observed; AppSDK
lifecycle commands and records remain authoritative for admission, review,
delivery, integration, promotion, freeze, and cleanup.

Feature and debug review are change-scoped. The candidate must bind evidence
that new or modified behavior preserves typed control truth, one owner and one
implementation, fixed lifecycle skeletons, configured operations, registered
hooks, declared gates, ablation before addition, and shared-function reuse.
Review reports untouched historical violations as advisory findings unless they
affect changed scope, safety, ownership, evidence truth, or required delivery.

`guide close` distinguishes `workflow_complete` from
`appsdk_lifecycle_complete`. A finished advisory plan cannot turn a draft or
blocked AppSDK lifecycle into PASS.

## 8. Closeout projector

`guide close` emits workflow state, remaining AppSDK lifecycle gaps, evidence
summary, cleanup requirement, and memory candidates. It never edits durable
rules or memory. Applying those candidates is a separate reviewed workflow.

## 9. Compatibility

- Projects without `guidance` continue to use every existing command unchanged.
- New projects receive advisory guidance declarations and the canonical Skill
  contract, and initialization prints the `guide compile -> guide init` route.
- Existing governed projects without Guide run idempotent `init`, read the
  bootstrap setup proposal, approve project-owned sources, and compile once.
- Binary byte hashes are not a guidance prerequisite. Version and project
  contract compatibility remain owned by existing AppSDK lifecycle commands.
