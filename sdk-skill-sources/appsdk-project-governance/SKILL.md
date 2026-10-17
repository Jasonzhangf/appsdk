---
name: appsdk-project-governance
description: AppSDK 项目治理、旧治理消融迁移和规则升级；release 前禁 SHA 与身份包装，按真实行为选测；协作与质量准入分开。
---

# AppSDK Project Governance

## Purpose and mandatory boundary

Use this Skill only when an AppSDK-managed project performs governance bootstrap, rule ablation/upgrade, `.appsdk/requirements.json` verification, or AppSDK-specific gate/lifecycle checks.

- **Pre-release SHA and wrapper ablation**: Follow global [`AGENTS.md`](../../AGENTS.md) L2 and [`coding-principals/references/workflow.md`](../coding-principals/references/workflow.md#消融与校验取舍). Before external release preparation, never compute or compare SHA/checksum digests, build candidate identity manifests, freeze snapshots, reconstruct patches byte-for-byte, or run `produce-lifecycle-chain` / `FreezeRecord` wrappers for ordinary development, review, or delivery.
- **Proportionate testing**: Select checks from the changed owner and concrete failure modes. Preserve red tests, real public-entrypoint black-box regression, and safety/data-correctness checks; physically delete tests or gates that only lock source text, private internals, or duplicate existing guarantees.
- **Collab vs. Quality separation**: Multi-agent Collab registration (`collab context`) coordinates shared work and never blocks independent single-worker implementation, testing, or quality gates. AppSDK governance state (`.appsdk/`, `.appsdk-control/`) and Collab runtime state (`~/.collab/`, `.agent-collab/`) have separate owners and separate reset/migration transactions.

## Core operational rules

1. **Authoritative requirements (`.appsdk/requirements.json`)**:
   - When `.appsdk/requirements.json` exists and is non-empty, read effective items via `appsdk requirements show` (and `history` when a change is claimed) before design, implementation, and milestone architecture review.
   - Missing or empty `.appsdk/requirements.json` is skipped without creating placeholder files.
   - Requirement changes require explicit user instruction; never weaken assertions or modify requirements to match a broken implementation.
2. **Bootstrap & rule upgrade (`Existing project: governance ablation`)**:
   - Global SDK installation (`scripts/install-global-appsdk.sh`) updates host binaries and global skills; it does not rewrite project `AGENTS.md`, hooks, or CI.
   - Upgrading an existing project to the 2026-10-16 ablated governance uses [bootstrap-migration.md](references/bootstrap-migration.md#existing-project-governance-ablation) to remove legacy SHA/freeze/identity-record calls from project docs, hooks, and gates. Never run `appsdk init --fresh` or `collab reset` merely to upgrade rules.
   - New project admission uses `appsdk prepare` -> explicit user confirmation of `.appsdk-prepare.json` -> `appsdk init` -> `appsdk verify`.
3. **Development, review, and delivery**:
   - Work in an exclusive clean external worktree (`/Volumes/Intel/playground/<project-key>/<task-slug>`) following the SESE Close-Before-Open lifecycle.
   - Check affected DAG completeness and SESE closure first (`dagpipe graph validate`), then run red test -> unique owner fix -> real public-entrypoint black-box regression.
   - Trigger one independent architecture/milestone review at the milestone integration boundary after author verification passes. Ordinary internal nodes and commits do not trigger review.

## Reference router (load on demand)

Read only the reference matching the current action; do not preload all files:

| Scenario | Reference |
| --- | --- |
| Existing project SHA/ceremony ablation, `appsdk prepare/init`, or legacy governance removal | [references/bootstrap-migration.md](references/bootstrap-migration.md) |
| Authoritative requirement review packet & milestone delivery/cleanup boundaries | [references/review-delivery.md](references/review-delivery.md) & [references/authoritative-review-template.md](references/authoritative-review-template.md) |
| AppSDK vs. Collab directory ownership (`.appsdk`, `.appsdk-control`, `.agent-collab`, `~/.collab`) | [references/state-paths.md](references/state-paths.md) |
| `appsdk` CLI command surface and exit semantics | [references/command-surface.md](references/command-surface.md) |
| Development & debug workflow mapping | [references/development-debug.md](references/development-debug.md) |
| Error contracts and failure classification | [references/contracts-and-failures.md](references/contracts-and-failures.md) |
| Host subagent / worker configuration and process harness | [references/subagents-config.md](references/subagents-config.md) & [references/process-control-harness.md](references/process-control-harness.md) |
| Initializer and long-horizon goal prompt templates | [references/init-prompts.md](references/init-prompts.md) & [references/goal-prompt.md](references/goal-prompt.md) |
| Control-plane migration, daemon maintenance, or authorized state reset | [`appsdk-migration`](../appsdk-migration/SKILL.md) |
