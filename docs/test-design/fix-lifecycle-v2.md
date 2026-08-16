# Fix Lifecycle v2 Test Design

## Lifecycle and whitebox

- Resolve every evidence ID and reject missing, duplicate, stale, wrong-module, wrong-scope, wrong-commit, or wrong-artifact evidence.
- Bind worktree base/head, candidate commit/tree/diff, architecture map hashes, effectiveness replay, and merged commit/tree.
- Require timestamps in causal order: worktree -> reproduction -> candidate -> review -> effectiveness -> merge -> promotion.

## Module blackbox

- Valid complete graph admits `architecture_stable`.
- Dirty worktree, base mismatch, incomplete evidence phases, stale review, changed candidate after review, mismatched replay input, missing mainline merge, and merged-tree drift fail closed.

## Project blackbox

- `appsdk verify`, module promotion, freeze, and Active publish enforce the same graph.
- Existing Active/Protected history remains immutable; a new version starts from `begin-version`.

## Known boundary

- AppSDK validates committed worktree facts and Git object/reachability facts. Host adapter owns worktree creation and write isolation.
