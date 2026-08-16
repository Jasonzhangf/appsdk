# Playground lifecycle

Playground means the mutable issue-resolution phase. Execute it in a clean isolated Git worktree created from the recorded base commit; it does not require copying the checkout under this directory.

Required order: reproduce the issue with recorded inputs, locate the first divergence, implement and commit one scoped fix candidate, verify positive and negative behavior, obtain architecture PASS for that exact commit/tree/maps, replay effectiveness without source changes, then verify mainline merge ancestry.

Runtime, release, Active libraries, and compiled artifacts must never consume mutable Playground source. Local absolute worktree paths belong only in `.appsdk-control/`; portable records store IDs, refs, commits, hashes, scope, and evidence.
