# Run Notes

## 2026-10-02T23:50:00-07:00 | baseline | confirmed

- Conclusion: assigned worktree is clean at `e4479efaee9be8817f52298812342ec1aa80a3bb` on `codex/a7cc2a4-pin-diagnosis-20261003`.
- Evidence: `git rev-parse HEAD`, `git status --short --branch`.
- Inputs: global AGENTS, `coding-principals`, `dagpipe-runtime`.
- Next: inspect `pin_lock` and migration validation.

## 2026-10-02T23:55:00-07:00 | reproduce | confirmed

- Conclusion: an archived genuine AgentTeams input reproduced `INVALID_SDK_MIGRATION_RECORD` through public `appsdk pin-lock`; it caused no fixture or AgentTeams tracked modification.
- Evidence: `fixture-genuine/` hashes in `report.md`; canonical binary `d7a67ef2...`; AgentTeams clean at baseline.
- Inputs: AgentTeams `de099b79f153e6fd220b98d5ee89f4777b7cfb53`, genuine `.appsdk` lock/record/snapshots.
- Caveat: the first fixture attempt omitted `.appsdk/maps`; public CLI returned `ARTIFACT_PATH_MISSING:governance_map`. Committed live maps were added before the successful reproduction.
- Next: bind the first rejected guard in source.

## 2026-10-03T00:05:00-07:00 | first divergence | confirmed

- Conclusion: first rejection is in `assert_sdk_migration_record`, target-authority comparison. The historical `canonical_target_digest` values differ from current embedded 0.1.6 target digests; source digests and snapshot files match.
- Evidence: `migration.rs:691-707`; current manifest `contracts/migrations/sdk-0.1.5-to-0.1.6.json`; genuine record and installed historical manifest.
- Inputs: canonical source binary and genuine fixture.
- Next: verify provenance and positive controls.

## 2026-10-03T00:15:00-07:00 | provenance | confirmed

- Conclusion: the genuine historical canonical target is authenticated by project-side frozen bundle evidence: installed `.appsdk/contracts/migrations/sdk-0.1.5-to-0.1.6.json`, its digest in `.appsdk/sdk-resources.json`, and the matching old bundle/ manifest digests in `.appsdk/sdk.lock`. It is not merely an unauthenticated custom target.
- Evidence: sdk-resources entry digest `03c37037...`, lock bundle `e9a8165...`, lock manifest `e83eb63...`.
- Next: run supported positive and negative controls.

## 2026-10-03T00:20:00-07:00 | controls | confirmed

- Conclusion: fresh current 0.1.0010 project pins and ordinary verify succeed; task-local source binary reproduces the genuine failure; existing synthetic historical-custom-target and preserved-custom-map public CLI tests pass.
- Evidence: `runtime/` command outputs recorded in report; tests `pin_lock_accepts_historical_custom_target_different_from_canonical_target` and `pin_lock_preserves_historical_custom_maps_with_bundle_witness` PASS.
- Inputs: task-owned `APPSDK_HOME`; task-owned `CARGO_TARGET_DIR`.
- Next: validate design DAG and identify minimal unique fix.

## 2026-10-03T00:13:00-07:00 | initial design graph | superseded

- Conclusion: the initial graph covered record validation and historical target admission but stopped short of the current-step custom-map materialization conflict.
- Evidence: `dagpipe graph validate` returned `valid DAG`; the graph is now superseded by the six-stage corrected graph below.
- Next: revise graph and report after the independent review.

## 2026-10-03T00:37:00-07:00 | report finalization | confirmed

- Conclusion: report finalized. Corrected the manifest-refresh attribution (record predates `a826766` and later refreshes `64c9708`/`0936b0f` produced the current targets), softened the installed-binary inference, added an explicit note that the graph is intentionally not registered in `docs/dagpipe/manifest.json` (outside allowed write scope), and verified every fixture file is byte-identical to baseline commit `de099b79` blobs.
- Evidence: fixture files SHA-256 MATCH against `git show de099b79:<path>` for all 15 inputs; `dagpipe graph validate docs/dagpipe/sdk-pin-history.graph.json` PASS; `git status --short --branch` shows only `docs/evidence/` and `docs/dagpipe/sdk-pin-history.graph.json`; task-owned `runtime/` absent; AgentTeams read-only tree clean.
- Inputs: base `e4479efaee9be8817f52298812342ec1aa80a3bb`; canonical binary `d7a67ef2...`.
- Next: parent review and independent design review before any product-code change.

## 2026-10-03 / primary evidence and resource audit

- Author turn.completed/exit0 and PID96570 absent. Assigned SDK HEAD/origin/main remain e4479ef; all product source unchanged. Static graph and diff checks PASS; genuine fixture and actual first guard/positive/negative public CLI evidence read. No product fix is implemented; corrected design still requires independent review + implementation + public CLI evidence.
- Positive-control sdk.bin matched canonical d7a67ef2 byte-for-byte. Removed only this rebuildable 13,985,424-byte test copy and recorded positive-control-binary-cleanup.json; retained genuine historical input, current config archive, worker event outputs, source identity and explicit recreate command. No live install/daemon/registry modified. Current-positive archive no longer represents a runnable installed project.

## 2026-10-03T01:28:40-0700 | second author correction | confirmed

- Conclusion: the corrected design has two required decisions at one migration owner: authenticate the historical record from its lock-witnessed old bundle and embedded historical manifest, then reconcile the current-step record with actually installed live maps. A historical-only allowlist would pass the first guard and fail the later current-step contract.
- Evidence: `.agent-collab/review/a7cc2a4-pin-design-20261003-r1/review.final.md`; source trace `migration.rs` `assert_sdk_migration_record` and `migrate_governance_maps`; current manifest `contracts/migrations/sdk-0.1.0009-to-0.1.0010.json`; corrected report.
- Inputs: base `e4479efaee9be8817f52298812342ec1aa80a3bb`; archived genuine fixture unchanged; no product source edit.
- Next: independent design review, then implementation and public CLI acceptance only.

## 2026-10-03T01:32:00-0700 | corrected graph and documents | completed

- Conclusion: report and graph now cover the full public path through historical provenance, canonical history reconciliation, current-step materialization, immutable retention verification, and final publication/retention outcome.
- Evidence: corrected `docs/evidence/a7cc2a4-pin-diagnosis-20261003/report.md`; `docs/dagpipe/sdk-pin-history.graph.json` validates; fixture digests unchanged.
- Inputs: independent r1 FAIL and direct source trace above.
- Next: parent acceptance/re-review. No source fix, install, merge, push, or upstream closure is claimed.

## 2026-10-03T02:12:00-0700 | owned second-stage public probe | completed

- Conclusion: the r1 second-stage hypothesis (`SDK_MIGRATION_TARGET_MAP_MISMATCH` after historical admission) is refuted by the public CLI on a task-owned full AgentTeams baseline copy. After changing only the copied historical record by setting each `canonical_target_digest` to its own historical `target_digest`, `pin-lock` exits 0, reaches SDK `0.1.0010`, preserves the four live custom maps, and writes the current-step record with `target_digest` equal to those live maps while retaining canonical provenance.
- Evidence: `runtime/probe-baseline-1` and `runtime/registry-baseline-1`; source binary `runtime/probe-target/debug/appsdk`; observed `pinned ...`; new record target hashes `44e8135a...`, `5a89f814...`, `b6c41f11...`, `21ae8fcd...`; full baseline `module-registry.json` SHA-256 `e02c62e5...`.
- Caveat: the copied record mutation intentionally bypasses the first guard and is not legitimate input. The committed 15-file `fixture-genuine` lacks the baseline `module-registry.json`; a copy from it stops at `MISSING_GOVERNANCE_MAP:module-registry.json`, so the full baseline archive is the valid probe. Ordinary `verify` on the probe later exits 1 with `INVALID_DECLARED_ZONE_CONTRACT`: the historical project declares the `zone-transition-manifest.json` alias while current pin publication installs `zone-transition.manifest.json` (`governance.rs:1280-1324`). This is a separate unresolved dependency for the green genuine `verify` path.
- Inputs: AgentTeams baseline `de099b79f153e6fd220b98d5ee89f4777b7cfb53`; AppSDK base `e4479efaee9be8817f52298812342ec1aa80a3bb`; no product source edit, install, or global registry change.
- Next: independent re-review. The corrected design now requires historical target authorization plus preservation of the observed current-step invariant, and explicitly cannot claim genuine green `verify` until the zone-contract alias dependency is closed.

## 2026-10-03T02:20:00-0700 | probe cleanup and final checks | completed

- Primary terminal intake correction: author59006 ESRCH/exit0/turn.completed. Enumerated every genuine archive file and compared bytes to its de099b79 Git blob: 14 files match, not the 15 in earlier summaries. Initial primary asserted 15 and failed only the count check; actual bytes were unchanged. Corrected report/receipt count, kept the historical observation below as history. Stage final author report/graph/notes and new probe receipt before exact r2 design review; no product or genuine-green claim. Probe runtime/cache is absent.

- Conclusion: synthetic probe facts are retained in `probe-consistency-receipt.json`; task-owned probe fixture copies, registries and build cache were removed; no product, install, daemon or global registry state changed.
- Evidence: `dagpipe graph validate docs/dagpipe/sdk-pin-history.graph.json` -> `valid DAG (6 nodes, 5 edges, 6 waves)`; report fence count even; receipt JSON valid; `git diff --name-only -- rust contracts scripts sdk-skill-sources templates collab docs/dagpipe/manifest.json` empty; `runtime/` absent; genuine `sdk.lock` and historical record hashes still match AgentTeams baseline `de099b79`.
- Inputs: retained report, run notes, receipts, fixtures and graph only.
- Next: parent handoff. No source fix, install, merge, push, upstream/Teams/U7 closure or independent review PASS is claimed.
