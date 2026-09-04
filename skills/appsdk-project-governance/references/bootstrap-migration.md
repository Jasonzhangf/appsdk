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

## Governance exists but Guide is missing

Do not reset or migrate valid lifecycle truth merely to add Guide. Run
idempotent initialization so the current AppSDK can install missing Guide
resources while preserving the existing project contract, maps, records,
Active, and Protected state.

An already governed root containing `.appsdk/project.json` may rerun
`appsdk init` directly; its existing project root is the authority, so a new
preparation record is not required for this non-destructive resource refresh.
Fresh or relocated initialization still requires confirmed preparation.

```text
appsdk init
-> appsdk guide status
-> GUIDANCE_SETUP_REQUIRED
-> appsdk guide init --task guidance-setup --mode bootstrap
-> Agent reads returned AGENTS and local Skill candidates
-> Agent asks only unresolved questions
-> Agent presents GuidanceSetupProposal
-> explicit user approval
-> clean owner worktree updates AGENTS/local Skill/machine contract/source declaration
-> appsdk guide compile
-> appsdk verify
```

Bootstrap intake is read-only. Candidate files are not compiled rule sources
until the user approves them and `.appsdk/project.json` declares them. A
task-level PlanProposal is not a substitute for this project-level setup and is
never copied into a Skill automatically.

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
-> if Guide is missing, complete the user-approved setup proposal first
-> run task guide init, read declared AGENTS/Skills, ask unresolved questions
-> place workflow at the current real phase
-> apply new rules to new/changed nodes
-> continue development
```

Untouched legacy gaps are warnings unless safety, source ownership, evidence
truth, or current delivery is affected.
