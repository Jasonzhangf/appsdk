# Frozen artifact rehydrate test design

## Lifecycle under test

`committed freeze graph + controlled frozen source -> generated projection -> Protected archive -> Active library -> verify -> begin-version`

The command is a projection rebuild owned by AppSDK lifecycle. It must not alter lifecycle records, invent hashes, accept a caller-provided version, or mutate frozen source.

For a 0.1.5 project, run the 0.1.6 binary's `pin-lock` first. The migration must reject unsupported source versions and a `--binary` whose bytes do not match the running Bundle owner before changing project truth. It must prove the exact 0.1.5 canonical map set and every frozen ReviewRecord map hash, persist one immutable historical snapshot, install the 0.1.6 canonical maps, and make historical review resolution depend on that exact record. A prior partial pin with project/lock already at 0.1.6 must resume the same migration rather than inventing a second path.

## Positive blackbox

- Freeze and publish a module through the public CLI.
- Remove the ignored generated and Active outputs plus the recoverable Protected projection to model a clean checkout of a legacy broken project.
- Run `appsdk rehydrate-frozen <project> --module <id>`.
- Require the rebuilt artifact hash to equal FreezeRecord and PromotionRecord. That signed artifact hash binds the computed public API hash; the existing record graph independently requires FreezeRecord, PromotionRecord, and RegressionReport API references to agree. Full `appsdk verify` must pass, then `begin-version` must open the exact next version.
- Freeze a second version, remove both Active versions and the current Protected archive, then require rehydrate to restore only the immediate previous Active from its versioned Protected archive before publishing the current version.
- Model a post-publication verification interruption with an exact `active_published` transaction marker and matching Protected/Active projections. A second invocation must verify and finalize without republishing or incrementing any attempt; a third invocation remains idempotent.

## Negative blackbox

- Commit controlled-source drift after the freeze, remove all local projections, and run rehydrate.
- Require an explicit hash mismatch, zero Protected/Active publication, and no successful verification.
- Reject partial or pre-existing target projections instead of overwriting them unless an exact transaction marker owns the same module/version/artifact hash. A complete exact final projection may be verified idempotently without a marker; this is completion recognition, not ownership inference for partial state.
- Resolve a cleaned recorded branch through exactly one matching remote-tracking ref; reject zero or multiple matches instead of guessing a remote.
- Reject mixed/drifted migration maps, a frozen review that does not bind the source snapshot, a changed snapshot after migration, and any current review that attempts to use historical map hashes.

## Whitebox boundaries

- `rehydrate_frozen` is the only orchestration entry.
- `stage_protected_archive` owns deterministic Protected construction.
- `restore_active_from_archive` restores an immediate previous Active only from its versioned Protected archive; `publish_active` remains the sole current-Active publication owner.
- `migrate_governance_maps` is the only SDK map migration writer; `assert_review_map_bindings` is the only historical hash resolver.
- `write_rehydrate_transaction`/`finish_rehydrate_transaction` own resumability; existing Active bytes are accepted only through `assert_active_projection_matches`.
- Build output is local projection only; committed lifecycle records remain read-only.

## Project blackbox impact

Fresh worktrees can legally continue a frozen module without copying ignored artifacts from another worktree. Protected must remain a durable, non-ignored project surface; V2 runtimes remain development-only until their own gates pass.
