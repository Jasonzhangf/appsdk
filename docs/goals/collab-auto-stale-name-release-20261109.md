# Collab auto stale-name release

## Goal

Make a released stable-name slot reusable by ordinary registration without
losing historical work or unrelated holders. The delivery owner is Jack in the
`codex/stale-name-integration-20261122` worktree.

## Progress

- 2026-11-22: Compared the original candidate with current `origin/main`. The
  current design no longer persists a free-name counter; it selects the smallest
  active `peer-<n>`, and `identity-map.json` keeps released slots as history.
- 2026-11-22: Added the allocator rule that only an active current binding
  occupies a generated name. The red test first selected `peer-2`; after the
  rule changed it selected the released `peer-1`.
- 2026-11-22: Added public AppServer evidence that `worker close --self`
  releases a full-pool `peer-1` and a later Context reuses `peer-1`. Recorded
  the reuse boundary in `collab/docs/verification-map.md`.
- 2026-11-22: Reconciled the original tree and this integration candidate.
  The original tree is clean at `d397c4ec` and is 176 commits behind
  `origin/main` with one commit ahead. This candidate is current-main with no
  commit divergence and five uncommitted paths. The old candidate's overflow,
  registration-number and self-naming semantics are superseded by the current
  main design; this candidate intentionally adds only released generated-name
  reuse. The old candidate's broader stale-absence maintenance removal, DAG
  edits, and requirements-version change remain unadopted and are not part of
  the current scope.

## Evidence

- Current-main candidate:
  `/Volumes/extension/playground/appsdk/stale-name-integration-20261122`,
  branch `codex/stale-name-integration-20261122`, commit `6ebba0121`. The
  candidate contains four modified files plus this task log.
- Owner task tree:
  `/Volumes/extension/playground/appsdk/stale-name-release-20261109`, branch
  `codex/stale-name-auto-20261109`, commit `d397c4ec`, clean status.
- Red: `released_peer_names_are_reused_by_the_next_registration` failed with
  `peer-2` before the allocator change.
- Green: `cargo test --manifest-path collab/Cargo.toml --locked --bin collab
  released_peer_names_are_reused_by_the_next_registration` passes (1 test).
- Green: `cargo test --manifest-path collab/Cargo.toml --locked --test
  appserver_two_tui_integration
  context_recovery_pool_overflow_registers_generated_names` passes (1 test).
- The candidate worktree is based on current `origin/main` with no commit
  divergence. `git diff --check` passes.
- Full `rustfmt --check` on the three Rust files reports pre-existing drift in
  `binding_slot.rs` and unrelated test modules; the formatting introduced by
  this change was corrected locally. No bulk formatting was run.
- Scope: `collab/src/server/binding_slot.rs`, allocator unit test, public
  AppServer regression, and verification-map row. `cargo fmt --check` reports
  pre-existing formatting in unrelated files and was not expanded into a bulk
  rewrite.

## Next

Jenny selected the short semantic owner path
`/Volumes/extension/playground/appsdk/stale-name-release-20261109`. The copied
owner tree was moved there with `git worktree move`; registration, branch,
commit, status and common-dir were verified. `collab task relocate` succeeded
and now reports the new owner path and branch `codex/stale-name-auto-20261109`.
The unregistered old Intel copy remains at
`/Volumes/Intel/playground/appsdk/stale-name-auto-20261109`; deletion was
rejected by the execution-tool safety gate and is retained until an approved
owner cleanup. Current-main integration evidence remains in
`/Volumes/extension/playground/appsdk/stale-name-integration-20261122`.

2026-11-22 review result: Tina was unavailable, so the authorized fallback ran
a read-only GCM Codex worker review (`gpt-6.2-sol`, mode `uncommitted`). The
controller completed it with exit 0, no findings, and PASS evidence at
`.agent-collab/review/stale-name-release-review-gcm-20261122/review.final.md`.
The formal Collab review command returned
`TASK_REQUIRED_REVIEWER: only the named reviewer's current holder may review
this task`; that lifecycle gate cannot represent the fallback worker and needs
master reconciliation. Engineering integration proceeds on the valid worker
PASS and does not wait for Tina.
