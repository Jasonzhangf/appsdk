# Development and Debug

## Feature or new project

Before implementation, close:

```text
requirements + acceptance + non-goals
-> confirmed scope
-> top-down module/resource map
-> high-level design
-> detailed call/data/error design
-> requirements/design/module/verification consistency
```

Then:

```text
latest origin/main
-> clean owner worktree
-> minimal implementation
-> candidate commit
-> verification/review/delivery
-> latest-main integration
-> remote receipt
-> lifecycle close
-> worktree/claim cleanup
```

Candidate commit binds tree/artifact/evidence. It is not a delivery commit and
does not authorize merge.

## Debug

Use one hypothesis per round:

```text
same-input reproduction
-> confirmation/falsification signals
-> first semantic divergence
-> forward intervention
-> reversal intervention when feasible
-> root-cause decision
-> unique-owner fix
-> old-input replay + regression
```

Final error is not automatically root cause. Grep hit is not evidence. Do not
patch output layers, add fallback, or modify multiple owners to make one test
green.

Before merge, check maps, module boundary, owner, payload/control separation,
duplicate implementation, affected tests, and the latest-main integrated tree.

