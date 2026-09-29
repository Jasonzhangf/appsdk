# Review and Delivery

Apply this runtime lifecycle when runtime delivery is in scope. Documentation
and rule edits use their relevant checks and review; they do not invent a
runtime deployment, Active artifact or freeze ceremony.

## Candidate to review

```text
candidate commit/tree/scope
-> development whitebox
-> exact artifact build
-> install receipt when required
-> restart receipt when required
-> deployed public-entrypoint blackbox
-> PreReviewValidationRecord
-> appsdk verify --review-admission
-> selected architecture review PASS
```

Review tool follows user choice; otherwise use configured default. Review is
read-only and bound to exact candidate, scope, maps, artifact, and evidence.

Required service operations come from module `deployment_operations`:
`["install", "restart"]`, `["install"]`, `["restart"]`, or `[]`. An omitted
field preserves the legacy install/restart requirement. The declaration is
artifact-bound; changing it invalidates existing validation. Every supplied
receipt is checked even when optional. Public-entrypoint blackbox and candidate,
artifact, environment and producer identity remain required. The legacy
`deployed_blackbox` label includes a library/CLI artifact's real consumer entry;
mock or source-only checks cannot stand in for that artifact.

Changed-scope architecture review must verify:

- control/configuration truth uses declared typed control resources, error
  chains, or project configuration sources and never business payloads,
  metadata, debug logs, or implicit context;
- each semantic behavior has one owner and one implementation, with no fallback
  or temporary bypass;
- a project-declared lifecycle skeleton preserves its owning boundaries;
- additions passed an ablation check and common semantics use one shared
  function;
- missing operators, hooks, or gates fail or skip explicitly and never mock
  success.

Concrete quality, safety, contract or material structural regressions block.
Optional simplification and design preferences are advisory. Untouched
historical violations are reported as recommendations and do not block unless
they affect changed scope, safety, ownership, evidence truth, or required
delivery.

Any source, test, build config, environment, artifact, scope, owner, or required
rule change invalidates affected evidence. Revise plan, rerun affected gates,
then review again.

## Review to mainline

```text
verify evidence freshness; reuse unchanged candidate evidence
-> fetch latest origin/main
-> exact integration build/test
-> protected merge/push
-> remote main receipt
```

Conflict returns to owner worktree. Do not resolve inside a serial merge queue
and keep stale review evidence.

EffectivenessRecord can reference pre-review candidate interventions and the
validated blackbox when input hashes, artifact and candidate identity match
and evidence has not expired. No mandatory rerun just because review finished.
Environment or dependency/configuration changes require affected evidence to
be refreshed. The full lifecycle still validates the current artifact and
pre-review graph before accepting reused effectiveness.

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

Resource close is separate from engineering delivery. Retain an owned worktree
with purpose recorded when needed; keep its cleanup obligation open. When
cleanup is authorized and required delivery/retention evidence exists:

1. archive required evidence;
2. create CleanupRecord or project equivalent;
3. remove only the owned merged worktree and branch;
4. verify removal;
5. release claim.
