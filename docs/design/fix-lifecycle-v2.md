# Fix Lifecycle v2

## Required order

```text
goal admitted
  -> clean isolated worktree from approved base
  -> baseline reproduction
  -> formal fix in the unique owner
  -> fix candidate verification
  -> architecture review PASS
  -> unchanged-candidate effectiveness replay PASS
  -> merge queue admission when parallel development is enabled
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

## Record graph

```text
WorktreeRecord
  -> ReproductionRecord
  -> EvidenceRecord[]
  -> FixCandidateRecord
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

Required evidence phases are `baseline_reproduction`, `fix_candidate`, `positive_intervention`, `negative_intervention`, and `post_architecture_effectiveness`. Every referenced evidence ID must resolve to a distinct record.

## Compatibility

Fix Lifecycle v2 began with the v0.1.3 single-worker graph. AppSDK v0.1.4 adds the atomic `multi_worker_collaboration` + `multi_worktree_merge_queue` scenario pair and keeps the graph fail-closed. The binary checks `.appsdk/project.json` and rejects a different SDK version with `PROJECT_SDK_VERSION_PIN_MISMATCH` before interpreting records. Older projects must use their retained versioned binary until an explicit migration creates verifiable records from repository history and retained evidence. Do not synthesize PASS records when historical evidence is unavailable.
