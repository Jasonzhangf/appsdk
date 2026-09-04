# Review and Delivery

## Candidate to review

```text
candidate commit/tree/scope
-> development whitebox
-> exact artifact build
-> install receipt
-> restart receipt when required
-> deployed public-entrypoint blackbox
-> PreReviewValidationRecord
-> appsdk verify --review-admission
-> selected architecture review PASS
```

Review tool follows user choice; otherwise use configured default. Review is
read-only and bound to exact candidate, scope, maps, artifact, and evidence.

Any source, test, build config, environment, artifact, scope, owner, or required
rule change invalidates affected evidence. Revise plan, rerun affected gates,
then review again.

## Review to mainline

```text
unchanged-source effectiveness
-> fetch latest origin/main
-> exact integration build/test
-> protected merge/push
-> remote main receipt
```

Conflict returns to owner worktree. Do not resolve inside a serial merge queue
and keep stale review evidence.

## Promotion and freeze

```text
RegressionReport on merged source
-> appsdk compile
-> publish immutable Active
-> archive source/contracts/artifact in Protected
-> FreezeRecord
-> appsdk verify
```

Merge alone is not lifecycle completion. Active/Protected are immutable; use
canonical version/open/rehydrate flows instead of manual edits or copies.

## Cleanup

After remote receipt, promotion/freeze requirements, and retention record:

1. archive required evidence;
2. create CleanupRecord or project equivalent;
3. remove only the owned merged worktree and branch;
4. verify removal;
5. release claim.

