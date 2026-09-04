# Bootstrap and Migration

## New project

```text
requirements + acceptance
-> appsdk prepare
-> confirm project root/boundaries/non-goals
-> appsdk init
-> appsdk guide compile
-> appsdk guide init --mode develop|debug
-> appsdk verify
-> clean owner worktree
```

`init` is idempotent. It fills missing governance resources and preserves
business files. Use `new` only for an empty destination.

## Existing project

Inventory AppSDK roots, maps, records, Active/Protected, local control state,
claims, and worktrees. Choose one route:

### Preserve and migrate

Use when historical evidence remains valuable and the current version has a
supported canonical migration.

```text
snapshot immutable truth
-> reconcile ownership/conflicts
-> run canonical migration once
-> verify one retained truth
-> compile Harness rules
```

### Reset and reinitialize

Use when old governance is obsolete, unsupported, or costs more than its audit
value. Reset is destructive and requires user authorization for the named
objects.

```text
inventory + immutable audit snapshot
-> classify retained business source and Protected artifacts
-> request exact reset/delete authority
-> remove only approved governance/runtime residue
-> appsdk prepare/init with current SDK
-> rebuild maps/goal/module/owner from current project truth
-> appsdk guide compile
-> appsdk guide init for the current task/domain
-> appsdk verify
```

Do not hand-edit version/hash/ReviewRecord to imitate migration. Do not retain
two active governance roots. Historical snapshots stay evidence; runtime state
may be zeroed only inside approved scope.

## Mid-development adoption

Do not force release/freeze evidence onto unfinished work.

```text
snapshot current source and task state
-> initialize advisory governance
-> bind current goal/module/owner/worktree
-> run guide init, read declared AGENTS/Skills, ask unresolved questions
-> place workflow at the current real phase
-> apply new rules to new/changed nodes
-> continue development
```

Untouched legacy gaps are warnings unless safety, source ownership, evidence
truth, or current delivery is affected.
