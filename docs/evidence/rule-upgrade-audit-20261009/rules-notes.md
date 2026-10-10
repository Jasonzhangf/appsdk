# appsdk-audit-rules notes

## Task and boundary

- Task ID: `rule-upgrade-audit-20261009`
- Worker: `appsdk-audit-rules`
- Baseline: `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`
- Workspace: `/Users/fanzhang/Documents/github/appsdk`
- Scope: read-only audit of all `sdk-skill-sources` Skills, references, machine guidance contracts, templates, contracts and current maps, install/CI scripts, README, and current architecture/rule docs.
- Allowed writes: this note and raw observations under the worker run directory only.
- Prohibited: source edits, tests, builds, installs, restarts, commits, pushes, agent dispatch, global-source edits, and long-term memory updates.

## Method and source baselines

- Read global method and ownership rules from `/Users/fanzhang/.agents/AGENTS.md`, `coding-principals/SKILL.md`, and `coding-principals/references/workflow.md`.
- Read the system `skill-creator` rules for progressive disclosure, single-owner text, and scope preservation.
- Read the parent run note and the prior `/Users/fanzhang/.codex/visualizations/2026/10/10/01a123de-ff54-76a2-9265-4f8f8d862230/sdk-rule-upgrade-proposal.md`.
- Source/install Skill comparison: `diff -qr` returned no differences for `appsdk-project-governance`, `appsdk-migration`, and `project-memory` between `sdk-skill-sources/` and `/Users/fanzhang/.agents/skills/`.
- Source Bundle manifest reports `0.1.0012`; installed Bundle directories found under `~/.local/share/appsdk` stop at `0.1.6`; no `0.1.0012` Bundle directory was present at observation time.

## Observation log

### 2026-10-09 rules inventory start

- Status: done. Full report written to `report.md` in this directory.

### 2026-10-09 batch: contracts / templates / install / CI (facts)

- Baseline re-confirmed: `git rev-parse HEAD` = `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`, clean worktree.
- Source version `0.1.0012` (`rust/release-version`, `contracts/sdk-bundle.manifest.json:4`); installed `appsdk 0.1.0012 (rust)` at `~/.cargo/bin/appsdk`.
- `contracts/sdk-bundle.manifest.json` resources match the embedded `SDK_BUNDLE_RESOURCES` array in `rust/src/main.rs` (105 literal + 1 constant `REVIEW_TEMPLATE_SOURCE` = 106). Every non-`skills/` path exists on disk; the `skills/` entries are install-relative and sourced from `sdk-skill-sources/**` (all present).
- `scripts/tests/test-install-global-appsdk.sh:90` pins `appsdk 0.1.0010 (rust)`; the script is referenced by no CI workflow, README, or other script (orphaned + version-stale).
- `.github/workflows/verify.yml` is the only workflow: format/test/release jobs run `cargo fmt`, `cargo test` (rust + dagpipe), `jq` contract validation, version-equality checks, and a `new`/`verify` bundle-layout smoke. It runs no `scripts/*.sh` and no `collab` tests.
- `docs/architecture/zone-transition-matrix.md:16` names `contracts/transitions/zone-transition-manifest.json` (hyphen) as the machine source; the actual machine data is `contracts/transitions/zone-transition.manifest.json` (dot) — the only transition file in the bundle manifest and the one embedded by `rust/src/main.rs:56`. The root hyphen file is a JSON Schema.
- `contracts/transitions/zone-transition.schema.json` is referenced by nothing except its own `$id`.
- `templates/minimal/contracts/transitions/zone-transition-manifest.json` is stale data (missing `development_whitebox_pass`, `deployed_blackbox_pass`, `pre_review_validation_pass`, `PreReviewValidationRecord`, `CollabLiveClosureRecordWhenParallel`) vs the canonical dot file; no test reads it.
- `~/.codex/skills` has no appsdk entries; `docs/migrations/appsdk-m1-identity-migration-20260910-inventory.md:22` says to retain a `.codex/skills/appsdk-project-governance` symlink; `scripts/install-global-appsdk.sh` writes only `~/.agents/skills` (line 46).
- `docs/design/appsdk-global-registry.md:28-30` calls `~/.local/share/appsdk` versioned bundles a deployment output "managed by the official installer"; no script writes there. Machine holds only historical dirs through `0.1.6`.
- A new project gets contracts in two roots: root `contracts/**` (from `bootstrap_contracts`, `rust/src/main/governance.rs:343`) and `.appsdk/contracts/**` (from `install_bundle_resources`). `contracts/maps/module-registry.json` is not in the bundle manifest although bootstrap writes `.appsdk/maps/module-registry.json`.
- `skills/appsdk-project-governance/SKILL.md` is installed twice per project (class `rules` -> `.appsdk/rules/appsdk-project-governance.md`, class `skills` -> `.appsdk/skills/.../SKILL.md`).
- Source Skills are byte-identical to installed `~/.agents/skills` counterparts (`diff -qr`).
