# Fix Lifecycle v2

## Required order

This is the runtime bug-fix lifecycle when its delivery/promotion phases are in
scope. Ordinary documents and local reviews do not run the full freeze chain.

```text
goal admitted
  -> clean isolated worktree from approved base
  -> baseline reproduction
  -> formal fix in the unique owner
  -> fix candidate verification
  -> development whitebox PASS
  -> build / applicable install and restart
  -> deployed public-entrypoint blackbox PASS
  -> pre-review validation PASS
  -> architecture review PASS
  -> unchanged-candidate effectiveness evidence PASS (reuse when valid)
  -> merge queue admission when that integration mode is enabled
  -> tested integration identity verification
  -> local and remote mainline receipt
  -> compile / Active / Protected / Freeze
```

Playground is the logical experiment lifecycle. The clean Git worktree is the physical execution boundary. Absolute local worktree paths stay in `.appsdk-control/`; committed records bind IDs, commits, tree/diff hashes, scope, producer, and timestamps.

## Invalidation

- Source, tree, diff, scope, owner, design, or map change after architecture review invalidates the review and effectiveness evidence.
- Effectiveness failure returns the lifecycle to `fix_candidate`.
- Candidate commit missing from the tested integration blocks promotion. In single-worker mode, the merged tree must equal the reviewed/effectiveness-tested candidate tree. In parallel mode, the final main tree may contain other accepted work, but the merged commit/tree must exactly equal IntegrationRecord and remain reachable from both local and recorded remote main refs.
- Compile and publish never consume Playground or an unmerged candidate branch.

## Phase re-entry and evidence reuse

Every phase is a separately persisted loop. The phase input identity includes
the candidate/tree, module scope, dependency records, artifact and environment
bindings, map hashes, evidence references, and the phase-specific mainline or
cleanup identifiers. AppSDK validates that identity and the upstream graph on
each call.

When an existing phase projection is PASS and its identity and evidence
freshness still match, the producer returns `reused: true` and skips the
external action for that phase. The read-only `verify` command may still walk
the full record graph to check hashes, references, timestamps, and expiry; that
walk does not rerun tests, deployment, merge, or publication. A reuse result is
therefore an exact cache decision, not fresh execution evidence.

Any candidate, dependency, map, artifact, environment, input, or freshness
drift invalidates that phase and every downstream phase. Immutable PASS records
remain as historical witnesses; AppSDK rejects the stale identity until a new
candidate-bound projection is produced. `fail`, `unknown`, malformed, or
expired projections never count as PASS. Repeating the same non-PASS identity
returns `LIFECYCLE_CHAIN_STAGE_NOT_PASS`; a changed identity archives the old
projection in append-only
`.appsdk/records/attempts/<module>/<phase>.jsonl` and re-enters the phase. A
partial Worktree/Reproduction/baseline record set, conflicting attempt history,
or tampered hash fails closed.

## Record graph

```text
WorktreeRecord
  -> ReproductionRecord
  -> EvidenceRecord[]
  -> FixCandidateRecord
  -> PreReviewValidationRecord (whitebox + applicable service receipts + public-entrypoint blackbox)
  -> Architecture ReviewRecord
  -> EffectivenessRecord
  -> CollaborationRecord (parallel)
  -> MergeQueueRecord (parallel)
  -> IntegrationRecord (parallel)
  -> MainlineReceiptRecord (parallel)
  -> MergeRecord
  -> PromotionRecord
  -> RegressionReport
  -> FreezeRecord
```

Required evidence covers baseline reproduction, candidate, positive and negative interventions, and public-entrypoint effectiveness. Effectiveness may reuse validated candidate interventions and `deployed_blackbox` with the same input/artifact and valid lifetime; alternatively use post-review replay. Each ID resolves to one record, and the same valid record can be referenced across phases without copying it. Review confidence is optional annotation.

The pre-review gate requires `development_whitebox` and `deployed_blackbox` for the exact candidate/artifact/environment. Module `deployment_operations` selects required install/restart receipts; omission preserves both, `[]` requires neither. Every supplied receipt is verified, and causality remains whitebox ≤ applicable receipts ≤ blackbox ≤ validation. The module declaration is artifact-bound. Issue lifecycle states such as `pre_review_validated` are derived record projections, not module promotion stages.

The WorktreeRecord captures the isolated worktree's observed starting head. A later FixCandidate may be a descendant of that head when the candidate remains bound to the same worktree, base commit, scope, evidence, and current source identity. The graph accepts that ancestry relation; it does not rewrite or delete the historical WorktreeRecord and still requires the candidate commit/tree and all review/effectiveness evidence to match exactly.

## Compatibility

Fix Lifecycle v2 began with the v0.1.3 single-worker graph. AppSDK v0.1.4 adds the atomic `multi_worker_collaboration` + `multi_worktree_merge_queue` scenario pair and keeps the graph fail-closed. AppSDK v0.1.5 adds pre-review whitebox/deployed-blackbox validation and evidence-backed install/restart receipts. A project already at `architecture_stable` must keep its retained binary until an explicit 0.1.5 migration can produce genuine PreReviewValidationRecord evidence bound to the historical deployed artifact, environment, receipts, and entrypoint. The binary checks `.appsdk/project.json` and rejects a different SDK version with `PROJECT_SDK_VERSION_PIN_MISMATCH` before interpreting records. Do not synthesize PASS records when historical evidence is unavailable.
