# Multi-worktree Merge Queue Test Design

## Lifecycle

- Scenario activation is all-or-none for multi-worker collaboration and multi-worktree merge queue.
- Every worker binds one exclusive semantic claim and one exclusive clean worktree.
- One merge owner serializes candidates.
- Candidate review/effectiveness precede queue admission.
- Integration gates run against the exact integration commit/tree.
- Local and recorded remote main refs must contain the integration commit before promotion or cleanup.

## Positive

- A candidate tree may differ from the integration tree when main advances.
- A clean integration commit with passing gates and local/remote reachability admits promotion.

## Negative

- Reject one-sided scenario activation.
- Reject shared/non-exclusive worktree or semantic claim.
- Reject queue/candidate/effectiveness identity mismatch.
- Reject conflict resolution in the merge queue.
- Reject a collaboration record that does not declare one independently verifiable milestone per worktree.
- Reject milestone identity drift between collaboration, queue, integration, receipt, and merge records.
- Reject a later milestone that lacks a predecessor remote receipt, reuses the predecessor worktree, or starts before the predecessor remote commit is in its base history.
- Reject failed integration gates or integration tree drift.
- Reject candidate not reachable from integration.
- Reject integration absent from local or recorded remote main.
- Reject missing/false remote verification receipt.

## Boundary

The host adapter writes the receipt, including remote name/ref, observed commit, producer, and observation time. AppSDK executes `git ls-remote` against that named remote/ref during `verify`; a mutable local tracking ref or self-declared boolean is not remote truth.
