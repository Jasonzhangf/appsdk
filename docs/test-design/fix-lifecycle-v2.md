# Fix Lifecycle v2 Test Design

## Lifecycle and whitebox

- Resolve every evidence ID and reject missing, duplicate, stale, wrong-module, wrong-scope, wrong-commit, or wrong-artifact evidence.
- Bind worktree base/head, candidate commit/tree/diff, current controlled source, rebuilt artifact identity, architecture map hashes, effectiveness replay, and merged commit/tree.
- Require timestamps in causal order: worktree -> reproduction -> candidate -> development whitebox -> install receipt -> restart receipt -> deployed blackbox -> pre-review validation -> review -> effectiveness -> merge -> promotion.

## Module blackbox

- Valid complete graph admits `architecture_stable`.
- Dirty worktree, base mismatch, incomplete evidence phases, stale review, changed candidate after review, mismatched replay input, missing mainline merge, and merged-tree drift fail closed.
- Missing or forged install/restart receipt evidence, whitebox or deployment producer adapter/identity drift, expired evidence, candidate tree mismatch, controlled-source drift before admission or architecture promotion, stale artifact, install/restart/blackbox causal-order drift, relabeled whitebox evidence, wrong deployed artifact/environment/entrypoint, or blackbox evidence created after review fails before architecture review admission.

## Project blackbox

- `appsdk verify`, module promotion, freeze, and Active publish enforce the same graph.
- `appsdk verify-sdk-source-registry` rejects unowned, multiply owned, or forbidden SDK source paths and is wired into CI before release.
- Existing Active/Protected history remains immutable; a new version starts from `begin-version`.

## Known boundary

- AppSDK validates committed worktree facts and Git object/reachability facts. Host adapter owns worktree creation and write isolation.
