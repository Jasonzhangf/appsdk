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
  -> mainline merge admission
  -> merged change identity verification
  -> compile / Active / Protected / Freeze
```

Playground is the logical experiment lifecycle. The clean Git worktree is the physical execution boundary. Absolute local worktree paths stay in `.appsdk-control/`; committed records bind IDs, commits, tree/diff hashes, scope, producer, and timestamps.

## Invalidation

- Source, tree, diff, scope, owner, design, or map change after architecture review invalidates the review and effectiveness evidence.
- Effectiveness failure returns the lifecycle to `fix_candidate`.
- Candidate commit missing from mainline, or the merged tree differing from the reviewed/effectiveness-tested candidate tree, blocks promotion. Rebase and repeat review/effectiveness when mainline moved; do not silently promote an expanded merge tree.
- Compile and publish never consume Playground or an unmerged candidate branch.

## Record graph

```text
WorktreeRecord
  -> ReproductionRecord
  -> EvidenceRecord[]
  -> FixCandidateRecord
  -> Architecture ReviewRecord
  -> EffectivenessRecord
  -> MergeRecord
  -> PromotionRecord
  -> RegressionReport
  -> FreezeRecord
```

Required evidence phases are `baseline_reproduction`, `fix_candidate`, `positive_intervention`, `negative_intervention`, and `post_architecture_effectiveness`. Every referenced evidence ID must resolve to a distinct record.

## Compatibility

Fix Lifecycle v2 is the v0.1.3 record graph and is intentionally fail-closed. The binary checks `.appsdk/project.json` and rejects a different SDK version with `PROJECT_SDK_VERSION_PIN_MISMATCH` before interpreting its records. A project frozen by v0.1.2 must execute the globally retained versioned binary (`~/.local/lib/appsdk/0.1.2/appsdk`) until a migration creates verifiable Worktree, Reproduction, FixCandidate, Architecture Review, Effectiveness, and Merge records from repository history and retained evidence. Do not synthesize PASS records when historical evidence is unavailable.
