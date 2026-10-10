# core audit notes

## Task

- Task: `rule-upgrade-audit-20261009`
- Worker: `appsdk-audit-core`
- Mode: read-only architecture audit. Product source and other workers' notes are read-only.
- Goal: audit AppSDK SDK core before rule/Skill upgrade, install, and release.
- Scope: `rust/src/main.rs`, `rust/src/main/**`, `rust/src/guidance*`, `rust/src/global_registry*`, relevant `rust/tests` cases, and current design documents.
- Required checks: bootstrap/init/upgrade, owner/DTO/validation/state duplication, missing links, declaration/implementation drift, test evidence invalidation, authorization reuse, proposal semantics.
- Forbidden: tests/build, install/restart, commit/push, agent dispatch, product edits, long-term memory promotion.
- Output: `core/notes.md`; final report is returned to the parent for `report.md`.

## Baseline

- Workspace: `/Users/fanzhang/Documents/github/appsdk`
- HEAD: `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`
- `origin/main`: `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`
- `git status --short`: clean
- Current source version: Cargo `0.1.12`; embedded `APPSDK_VERSION` is supplied by build metadata and must be checked against the installed binary by the runtime auditor.

## Scope inventory

- CLI/entry: `rust/src/main.rs`, `rust/src/main/cli.rs`
- `main/**`: 36 Rust modules covering init, migration, compile, registry, promotion, reset, review/merge/verification, goal, bug, and lifecycle producers.
- `guidance/**`: root dispatcher plus compiler, intake, ledger, projector, review, and help.
- Global registry: `global_registry.rs`, `global_registry_communication.rs`, `global_registry_tests.rs`.
- Relevant tests: `rust/tests/cli_smoke/**`, migration tests, communication schema tests, and in-module registry/guidance tests.
- Design docs: `docs/design/appsdk-guidance-framework.md`, `appsdk-global-registry.md`, `appsdk-project-integration.md`, `appsdk-authoritative-review-packet.md`, architecture and lifecycle documents.

## Progress

- 2026-10-09 baseline and inventory recorded.
- 2026-10-09 extracted module/function skeletons and CLI dispatch candidates.
- 2026-10-09 traced CLI dispatch for `init`/`new`/`pin-lock`/`guide *`/`requirements`; read `main/init.rs`, `main/migration.rs`, `main/reset_governance.rs`, `main/project.rs`, `main/governance.rs`, `main/requirements.rs`, `guidance.rs`, `guidance/{intake,compiler,ledger,projector,review}.rs`, `global_registry*.rs`.
- 2026-10-09 verified `install_bundle_resources` writes bundle contracts + `.appsdk/sdk-resources.json` unconditionally; `write_current_sdk_lock` early-returns on version mismatch; `migrate_governance_maps` only reachable via `pin-lock`. Existing tests only cover same-version `init` refresh (`part_24.rs:456`) and `pin-lock` migration (`sdk_0008/0009_migration.rs`); no test exercises plain `init` on a mismatched SDK pin.
- 2026-10-09 verified `GuidanceSetupProposal` has no apply/approve consumer; `guide` dispatcher exposes compile/status/init/plan/update/next/close/tour/review only.
- Next: finish guidance review/ledger semantics and global-registry DAG; consolidate findings.

## Findings

### F1 (static, confirmed call-chain) Plain `init` on a stale SDK pin partially refreshes the bundle before failing verify

- Entry: `rust/src/main.rs:1453-1455` routes an existing `.appsdk/project.json` to `init_project(root, false, false)`.
- Owner: `rust/src/main/init.rs:842-898` runs `ensure_governance_layout` -> `write_project_scaffold` -> `install_bundle_resources` -> `write_current_sdk_lock` -> `install_standard_template_reference` in that order.
- `install_bundle_resources` (`rust/src/main/governance.rs:247-296`) unconditionally rewrites `.appsdk/contracts/**`, `.appsdk/docs/**`, `.appsdk/skills/**` and `.appsdk/sdk-resources.json` with the current bundle.
- `write_current_sdk_lock` (`rust/src/main/init.rs:82-84`) returns early when `/sdk/version != SDK_VERSION`, so the stale `.appsdk/project.json`/`sdk.lock` are preserved while the bundle content is current.
- Consequence: project pin version stays old, bundle content is new; `verify` then fails `PROJECT_SDK_VERSION_PIN_MISMATCH` (`rust/src/main/project.rs:85-95`), and the on-disk `.appsdk` is non-transactional (mixed-version bundle vs pin).
- Minimal revision candidate: check the SDK pin before any bundle write; on mismatch fail closed with an explicit migration-required code pointing at `pin-lock`, or route the whole refresh through the migration transaction. Do not make plain `init` perform the destructive/frozen migration implicitly.
- Preserve: `pin-lock` remains the sole migration owner (`rust/src/main/reset_governance.rs:3-140`).
- Targeted verification entry: CLI smoke that creates a project, rewrites `/sdk/version` to an older pin, runs plain `init`, and asserts either a typed failure before any SDK-owned resource mutation or a fully consistent migration.
- Status: static call-chain finding. No test/build/runtime executed.

### F2 (static, confirmed; conditional on a version bump) The pin-lock migration chain has no refresh path for a new SDK version

- The migration step set is closed: `rust/src/main/reset_governance.rs:13-21` accepts only `0.1.3..0.1.0012`, and the loop at `reset_governance.rs:53-83` hardcodes steps up to `"0.1.0011-to-0.1.0012"` plus the final `migrate_governance_maps(..., "0.1.0011-to-0.1.0012")`.
- `sdk_map_migration_manifest` (`rust/src/main/governance.rs:82-104`) has no entry for a `0.1.0012-to-<next>` step; `historical_governance_map` (`governance.rs:6-80`) has no `0.1.0012` snapshot.
- `migrate_governance_maps` early-returns when the step target version != current SDK version (`rust/src/main/migration.rs:841-847`), so once `SDK_VERSION` advances past `0.1.0012`, the only map-refresh path stops materializing the new canonical maps.
- Consequence for the planned upgrade: bumping `rust/release-version` alone leaves governance maps unreconciled to the new bundle, and `pin-lock` fails `UNSUPPORTED_SDK_MIGRATION`. A new `0.1.0012-to-<next>` step, a `0.1.0012` historical snapshot, updated step/version lists, and the final map-refresh call are all required together.
- Minimal revision candidate: add the single next migration step + snapshot and extend the version lists in one change; do not widen the framework.
- Targeted verification entry: a `pin-lock` smoke that starts from a `0.1.0012` project and asserts the new maps/record; plus `appsdk verify` after migration.
- Status: static. Conditional on the release bumping `SDK_VERSION`; if the version is not bumped, this does not trigger.

### F3 (static, confirmed) Test evidence is hard-pinned to `0.1.0012`, so it invalidates itself on the same upgrade

- `rust/tests/sdk_0009_migration.rs:43-45` asserts `lock["version"] == "0.1.0012"` and no `0.1.0011-to-0.1.0012` record.
- `rust/tests/cli_smoke/part_24.rs:306-311`, `part_23.rs:44-46`, `part_01.rs:1188-1300`, `part_07.rs:371`, `part_08.rs:1318-1358`, `part_10.rs:968`, `part_02.rs:557`, `part_03.rs:607`, `part_22.rs:142` all assert the literal `0.1.0012` (lock version, project version, embedded bundle string, intake template version, migration record path).
- Consequence: the moment `release-version`/`SDK_VERSION` changes, these assertions fail by construction, so any "all tests pass" evidence from this baseline does not carry to the upgraded version and must be regenerated with the version literals updated alongside the F2 migration step.
- Minimal revision candidate: update the pinned literals in the same commit as the version bump; keep the `0.1.0011`/`0.1.0012` migration assertions that intentionally test historical steps.
- Status: static. No test executed.

### F4 (semantics boundary, not a defect) `GuidanceSetupProposal` is advisory-only; approval is not a machine gate

- `rust/src/guidance/intake.rs:217-366` builds the proposal object with `approval_required`, `after_user_approval`, and string commands; it does not persist an approval record or a typed apply request.
- `rust/src/guidance.rs:922-1018` exposes no `setup apply`/`approve` command; `guide compile` (`rust/src/guidance/compiler.rs:133-267`) validates declared sources and machine contract only.
- Repo-wide, no consumer parses a proposal, stores an approval, or checks approval before a durable rule write.
- Design intent matches: `docs/architecture/development-process-control-harness.md` defines it as a human-approved project-level proposal and states AppSDK does not interpret prose or keep a second intake truth.
- Risk to state explicitly: the SDK cannot prove that a durable rule change corresponds to a specific proposal/approval; `approval_required` is a collaboration instruction, not an enforced gate.
- Minimal recommendation: keep the human boundary but make the docs state that approval evidence is not SDK-enforced; only add a typed approval/apply record if the release contract requires auditable authorization reuse.
- Status: static read. No runtime verification.

### F5 (minor, static) `flow_review` takes an unused workflow parameter

- `rust/src/guidance/review.rs:414` binds `_workflow: &Value` and never reads it; node/edge validation uses the caller-supplied `path` and `path_set` only.
- Impact: no behavior change; it is dead parameter plumbing, not a missing check (the caller already validates `assert_path` at `review.rs:626`).
- Minimal revision candidate: drop the parameter when the call site is next touched; not worth a standalone change.
- Status: static. No defect.

## Unverified boundaries

- No test/build/install/runtime commands are allowed in this worker. All runtime and installed-binary conclusions remain for the runtime auditor.
- Static inspection only. No finding is a product or test PASS.
