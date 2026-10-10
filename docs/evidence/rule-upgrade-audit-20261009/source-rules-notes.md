# source-rules notes

## Task and boundary

- Task: `rule-upgrade-audit-20261009`, feature `aabed6a`
- Worker: `source-rules` (scope B)
- Baseline: `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`
- Worktree: `/Users/fanzhang/Documents/github/appsdk`
- Run: `/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/source-rules`
- Allowed implementation: `sdk-skill-sources/**`, `templates/minimal/AGENTS.md`, related design/architecture/rule docs, and consumer-confirmed removal of the stale template transition copy and orphan schema.
- Prohibited: commit, merge, push, install, restart, global/installed Skill edits, shared configuration, governance registration, product-repository init, Rust/intake/version/manifest edits, and product build/test execution.

## Accepted plan

1. Add the minimal SDK/template upgrade audit rule to the project template.
2. Give all three SDK Skills a clear applicability boundary without repeating full workflows.
3. Keep SDK-only rules/Skill/template upgrades separate from Collab daemon and identity migration; retain owner references and necessary authorization/invariant constraints.
4. Fix stale `collab reset` and `appsdk reset-governance` examples.
5. Remove only the stale template transition copy and orphan schema after consumer confirmation; keep the canonical dot contract, root compatibility alias, and project-owned module registry.
6. Run static checks only. Controller owns combined-tree tests, install, review, integration, and release.

## Evidence and consumer check

- `git status --short` was empty and `git rev-parse HEAD` returned the accepted baseline.
- `contracts/transitions/zone-transition.manifest.json` is embedded by `rust/src/main.rs` and listed by the bundle manifest.
- `templates/minimal/contracts/transitions/zone-transition.manifest.json` is read by `rust/tests/cli_smoke/part_17.rs`; it stays.
- `templates/minimal/contracts/transitions/zone-transition-manifest.json` has no code/test consumer. The only reference is the obsolete `zone_transition_manifest` member in `templates/minimal/.appsdk/project.json`.
- `contracts/transitions/zone-transition.schema.json` has no reference except its own `$id`.
- The root `contracts/transitions/zone-transition-manifest.json` remains a supported project declaration and is consumed by Rust migration/governance code; it stays.
- The installed `collab` Skill and live `collab reset --help` both require exactly one of `--project` or `--host`.
- Live `appsdk reset-governance --help` is `appsdk reset-governance [project] --discard-legacy`; project examples should pass the project path explicitly.

## Nodes

### 2026-10-09 implementation start

- Status: implementation and static self-check complete.
- Changes: added the template/Skill upgrade-audit boundary, separated SDK-only
  upgrades from Collab migration, fixed reset examples, aligned related docs,
  removed the stale template transition copy and orphan schema, and removed the
  obsolete template reference.
- Validation: three `quick_validate.py` runs passed, `git diff --check` passed,
  template project JSON parsed, and no current-tree references remain to the
  deleted paths.
- Boundary: no Cargo test/build, installer, install, daemon, or runtime replay
  was run; controller owns the combined release gates.
- Patch: `patch.diff` generated with `git diff --binary`; reverse-apply check
  passed.
- Status: DONE. Stop after handoff.
