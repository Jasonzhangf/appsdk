---
name: appsdk-project-governance
description: Apply AppSDK engineering quality gates, project contracts and lifecycle evidence. Use optional Guidance for planning; keep automatic multi-worker Collab separate from quality admission.
---

# AppSDK Project Governance

## Purpose and mandatory boundary

AppSDK verifies engineering quality. Collab supports automatic multi-worker
registration, communication and task/file ownership. Memory and Guidance help
when useful. Missing auxiliary state does not fail independent development.

Default flow: understand goal/scope → implement → relevant verification →
review → authorized delivery. Require applicable quality, safety and evidence
integrity gates; do not turn every available command into a mandatory phase.

Run project commands from project cwd. An explicit optional project path is for
operators intentionally working elsewhere; no project-root environment variable.

## Working loop

1. Read project AGENTS and affected code/contracts. Resolve owner, scope,
   acceptance and relevant gates. Read historical notes only when they help.
2. Use a clean owner worktree from latest origin/main. Preserve others' work.
   For multi-worker work, automatically register the peer, communications and
   task/file scope through official Collab; enforce overlap/resource ownership.
3. Implement the smallest adequate change. Use existing design for local work;
   clarify only material unknowns. Do not require a new plan or approval when
   scope is already authorized and clear.
4. Run applicable tests, necessary build and actual entrypoint checks. Install
   and restart only when required by the delivery object. Fix failures at their
   owner, never forge evidence or hide errors.
5. Review exact validated changes under the shared review standard. Block
   concrete correctness, safety, contract or material structural regressions;
   optional simplifications are advisory.
6. Reuse still-valid evidence when relevant source, inputs, dependencies,
   configuration, artifact and environment remain unchanged. Rerun affected
   checks after changes; verify an altered integration candidate.
7. Deliver within authorization. Report test, review, merge, install, publish
   and resource cleanup as separate achieved states.

## Optional Guidance

Use `appsdk guide status/init/plan/update/next/close` when the user/project
selects persistent planning or a long task benefits from recovery. Default
`advisory` and `warning` do not require a task plan or setup before development.
Missing PlanRecord does not fail ordinary `verify` or `compile`.

When using Guidance, follow its declared transitions and bind observations to
the current context. A failed optional workflow is not a failed quality gate.
Do not fabricate a successful step to close a plan.

For a requested setup/upgrade, `guide init --mode bootstrap` is read-only.
Compare current project-owned sources with the advisory standard template;
apply only authorized rule changes. `appsdk init` refreshes SDK resources but
never overwrites project AGENTS, Skills, records, Active or Protected. Merely
auditing rules does not require running initialization or changing setup.

## Automatic Collab

`appsdk init` attempts official `collab init` once in a live tmux peer, preserving
the inherited environment. Successful initialization registers identity and the
finite direct-message subscription. Do not duplicate that initialization.
Once task/worktree scope is known, use the Collab task lifecycle to register
feature/resource and file ownership before concurrent edits. Do not invent
task scope inside AppSDK initialization.

No tmux peer means pending. An unavailable/failed Collab reports its error;
independent work continues, while operations requiring shared ownership wait.
Keep automatic communication and file/task collaboration enabled; a serial
merge queue is required only when the project selects that integration mode.
Its ownership and tested-integration protections remain mandatory.

## Evidence and state ownership

- Project AGENTS owns project facts; Skills own procedure; declared machine
  contracts own enforceable gates. Existing lifecycle records remain the sole
  evidence truth. Plans, notes and Collab statuses do not duplicate PASS.
- Runtime review admission retains whitebox, public-entrypoint blackbox and
  exact candidate/artifact/environment identity. Module `deployment_operations`
  declares required `install`/`restart` receipts; omission retains both for
  compatibility, `[]` means neither operation applies. Bind this choice before
  validation; changes invalidate artifact identity. Every supplied receipt is
  checked. A missing required capability remains a blocker.
- Review confidence scores are optional annotation, never proof of quality.
- Freeze/Active/Protected apply when immutable artifact publication is in
  scope. Do not require freezing for a documentation edit or ordinary review.
- Engineering delivery may complete with a retained worktree. Keep ownership
  and cleanup obligations explicit; only claim resource closure after actual
  safe cleanup. No forced deletion to make a task appear complete.
- Memory is optional. No automatic durable memory/rule promotion. Long tasks
  and handoffs may record concise decisions and references to existing evidence.
  Memory migration and re-entry are explicit independent operations: use
  `project-memory migrate` for a source-preserving, resumable schema move and
  `project-memory reentry [project] --run <run-id>` to resume the same run after
  interruption. A missing or rebuilding memory index is not a governance
  failure, and memory state must not be reconstructed from Guide, debug,
  develop, or log payloads.

## References: load only the relevant domain

- Initialization or migration: [bootstrap-migration.md](references/bootstrap-migration.md).
- Development/debug: [development-debug.md](references/development-debug.md).
- Runtime review/delivery/freeze: [review-delivery.md](references/review-delivery.md).
- Selected persistent planning: [process-control-harness.md](references/process-control-harness.md).
- Contract errors/compatibility: [contracts-and-failures.md](references/contracts-and-failures.md).
- Explicit goal-prompt request: [goal-prompt.md](references/goal-prompt.md).

Failure reports name the failed applicable gate, preserved state, owner and next
action. Never infer deployed success, merge, freeze or cleanup from an earlier
test or an auxiliary workflow close.
