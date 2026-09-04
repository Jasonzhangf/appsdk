---
name: appsdk-project-governance
description: Bootstrap, migrate, plan, execute, verify, review, deliver, freeze, and clean projects through AppSDK. Use for AppSDK governance, Development Process Control Harness, persistent agent plans, project lifecycle evidence, or governance recovery.
---

# AppSDK Project Governance

## L0 Goal

Use external AppSDK as governance engine. Keep project contracts and records in
the project. Use one Development Process Control Harness to guide the agent
across the full lifecycle.

Governance helps delivery. It must not block unrelated work through binary byte
identity, old optional metadata, or release evidence that the current phase does
not require.

## L1 Entry loop

1. Read project `AGENTS.md`, `note.md`, current run notes, project `MEMORY.md`,
   `.appsdk/project.json`, maps, and relevant records.
2. Identify goal, module, owner, allowed paths, forbidden paths, lifecycle stage,
   required evidence, and clean owner worktree.
3. Run `appsdk guide status <project> [--task <id>]`.
4. If status returns `GUIDANCE_SETUP_REQUIRED`, run
   `appsdk guide init <project> --task guidance-setup --mode bootstrap --module <id>`.
   Read every returned project document and candidate Skill, ask only unresolved
   questions, and present one `GuidanceSetupProposal`. Do not write or compile
   durable rules before explicit user approval.
5. After approval, update the project-owned `AGENTS.md`, local Skill, machine
   guidance contract, and `.appsdk/project.json#/guidance/rule_sources` in a
   clean owner worktree. If already configured but uncompiled, skip setup and
   run `appsdk guide compile <project>` once.
6. Select one domain: `bootstrap`, `migration`, `governance-preflight`,
   `develop`, `debug`, `review`, `delivery`, `integration`, `promotion`,
   `freeze`, or `cleanup`.
7. Run `appsdk guide init <project> --task <id> --mode <domain> --module <id>`.
   Read the returned AGENTS/Skill paths in precedence order, invoke the
   suggested Skill commands, and ask the user only questions still unresolved.
8. Run the projected domain command. Let the agent write PlanProposal JSON and
   submit it with
   `appsdk guide plan <project> --task <id> --input <file>`.
9. Execute only the projected step. Submit observation/evidence with
   `appsdk guide update <project> --task <id> --input <file>`.
10. Read `appsdk guide next`; revise the plan when scope, owner, source, rule
   context, evidence, blocker, or environment changes.
11. Finish with `appsdk guide close`; then complete canonical lifecycle and
   worktree/claim cleanup. Workflow completion is not lifecycle completion.

## L2 Hard boundaries

- AppSDK never calls a model. Agent authors technical plans; Harness validates,
  persists, projects state, and returns adjacent next steps.
- `guide init` is read-only. Before initial compile, bootstrap mode projects
  bounded project-document candidates and a setup proposal schema. After
  compile, it projects declared context, interactive questions, Skill
  invocations, and missing/next commands. `guide plan` is the first task state
  write.
- Existing AppSDK lifecycle is sole truth. No second lifecycle enum or manual
  PASS/record/hash/artifact.
- `AGENTS.md` owns project facts. `SKILL.md` owns agent procedure. Declared JSON
  owns machine nodes, edges, gates, severity, and evidence contracts.
- Compiled and task guidance reads only rule sources explicitly declared by
  `.appsdk/project.json`. Bootstrap may discover only root `AGENTS.md`, the
  bundled AppSDK Skill, and one-level project-local Skills under `skills/`,
  `.agents/skills/`, or `.codex/skills/`; these remain candidates until the user
  approves and the project declares them.
- Guidance defaults to `advisory`. Missing PlanRecord never fails ordinary
  `appsdk verify` or `appsdk compile`.
- `forbidden` stays narrow: fabricated evidence, non-adjacent transition,
  history overwrite/delete, bound-context drift, main mutation, or false
  review/delivery/promotion/freeze/cleanup completion.
- Binary byte hash is not a governance admission gate. Use the selected AppSDK
  version/contract; lifecycle compatibility errors expose a migration/reset
  route.
- Code and committed governance changes use a clean branch worktree from latest
  `origin/main`. Main stays read-only. Preserve other workers' dirty state.
- Completed claim must bind cleanup evidence. Remote receipt and required
  retention first; then remove owned worktree/branch and release claim.
- No automatic retry, polling storm, fallback, downgrade, or automatic durable
  memory/rule write.
- A task `PlanProposal` never becomes a project Skill automatically. Promote a
  reusable procedure only through a separate user-approved governance change.

## L3 Domain routing

- Bootstrap or old state: read
  [bootstrap-migration.md](references/bootstrap-migration.md).
- Feature or project development: read
  [development-debug.md](references/development-debug.md).
- Bug, regression, or incident: read the debug section in that same reference.
- Candidate, review, deployment, merge, Active, Protected, or freeze: read
  [review-delivery.md](references/review-delivery.md).
- Harness Plan/Update/Next/Close: read
  [process-control-harness.md](references/process-control-harness.md).
- Gate classification, structured failures, cleanup, or compatibility: read
  [contracts-and-failures.md](references/contracts-and-failures.md).
- `/goal` prompt request: read [goal-prompt.md](references/goal-prompt.md).

## L4 Evidence contract

State exact evidence achieved: source, test, build, installed artifact, restart,
deployed entrypoint replay, review, merge, remote receipt, freeze, cleanup.
Never collapse these levels.

Debug evidence includes hypothesis, confirmation/falsification signals, first
divergence, experiment, forward/reversal result, root cause, and regression.

Every blocked result includes first failing gate, project/module/lifecycle
state, preserved state, retry permission, owner, and one executable next action.
Generic refusal is invalid.

## L5 Canonical references

- Architecture: `.appsdk/docs/architecture/development-process-control-harness.md`
- Detailed design: `.appsdk/docs/design/appsdk-guidance-framework.md`
- Machine workflow: `appsdk-guidance.json`
- Project integration: `.appsdk/docs/design/appsdk-project-integration.md`
