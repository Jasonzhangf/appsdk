# Bootstrap and Migration

## New project

```text
requirements + acceptance
-> appsdk prepare
-> confirm project root/boundaries/non-goals
-> appsdk init
-> optional approved Guidance setup/compile
-> appsdk verify
-> clean owner worktree
```

`init` is idempotent. It fills missing governance resources and preserves
business files. In a live tmux Agent it also invokes official `collab init`
once; Collab starts/reuses its daemon, registers the current peer, and arms the
default finite `direct-message` subscription. Do not run `collab init`,
`collab whoami`, or a manual ordinary-message subscription afterward. Use
`new` only for an empty destination.

AppSDK preserves the launching environment and does not pass a project path to
Collab. Collab resolves project scope from tmux pane cwd. Without a live tmux
pane, AppSDK initializes governance and reports Collab pending because no peer
can be registered; it never fabricates subscription state.

Collab initialization errors are explicit warnings for AppSDK initialization.
Automatic multi-worker registration and task/file coordination remain enabled;
shared operations wait for reliable ownership while independent work continues.

For a new governance root, AppSDK installs a project-neutral root `AGENTS.md`
when none exists. It contains the Project Truth, Semantic Invariants,
Ownership, Architecture Truth, Development Process Control, Git Protection,
Task Routing, and Evidence Boundary sections used by Guide setup. Customize
the bracketed project facts through the approved setup flow. Existing project
rules are never overwritten, and rerunning `init` on an already governed root
does not recreate a deliberately absent `AGENTS.md`.

## Repeat initialization and template upgrade

Initialization is not one-shot. After AppSDK is updated, or when the project
wants to revisit its process, rerun `appsdk init` to refresh AppSDK-owned Bundle
resources. It installs the current versioned standard reference at
`.appsdk/templates/minimal/AGENTS.md` while preserving the project-owned
`AGENTS.md`, local Skills, machine Guidance, lifecycle records, Active, and
Protected state.

```text
appsdk init
-> appsdk guide init --task guidance-upgrade --mode bootstrap --module <id>
-> Agent reads current AGENTS/Skills/Guidance first
-> Agent reads the versioned standard template reference
-> compare retained rules and useful differences
-> GuidanceSetupProposal
-> explicit user approval
-> latest origin/main clean owner worktree
-> apply only approved changes
-> appsdk guide compile
-> appsdk verify
```

The template is a standard reference, not an active rule source. Do not add it
to `.appsdk/project.json#/guidance/rule_sources`, automatically overwrite
project rules, or reset valid governance merely to adopt a newer template.
The proposal records retained project rules, recommended changes, and declined
template items so choosing not to adopt an item is explicit and valid.
Missing or locally removed reference material does not fail ordinary
`appsdk verify`; rerun `appsdk init` only when a fresh comparison is wanted.

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

Bootstrap intake is read-only and may be invoked again after Guidance has been
compiled. Candidate files are not compiled rule sources
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
-> clean non-main owner worktree
-> appsdk reset-governance --discard-legacy
-> old .appsdk audit/migration records and generated projection removed
-> appsdk init with current SDK
-> rebuild maps/goal/module/owner from current project truth
-> appsdk guide compile
-> appsdk guide init for the current task/domain
-> appsdk verify
```

The reset command is idempotent and preserves business source, runtime data,
Active, and Protected by default. It removes stale audit/migration reports,
control state, and rebuildable generated output. If old Active/Protected
artifacts are also obsolete, name exact paths and authorize a separate
cleanup; reset must not silently delete them. Do not hand-edit
version/hash/ReviewRecord to imitate migration, and do not retain two active
governance roots. The new reset record proves the reset operation only; it does
not inherit old PASS, review, delivery, or freeze claims.

### What to do with old reports and delivery output

Use ownership and rebuildability, not age, to decide what is removable:

| Class | Default action | Reason |
| --- | --- | --- |
| `.appsdk/records`, `.appsdk/transactions`, audit/migration reports | Inventory/snapshot if needed, then remove through reset | Old control truth must not leak into the new baseline. |
| Declared `governance.generated_root`, module generated outputs | Remove through reset and regenerate | These are reproducible projections, not source or release truth. |
| Failed transaction staging | Canonical abort/retry if current; otherwise reset | Manual deletion can hide ownership or partial publication. |
| `active/`, `protected/`, runtime data, business source | Retain | They may be the only published or operational truth. |
| `dist/`, `.deploy/`, `build/`, `tmp/`, custom reports/artifacts | Keep until exact disposable ownership is confirmed | AppSDK cannot infer that an external output is safe to delete. |

The reset command reads the old project contract before removal and includes
its declared generated root in the disposable set. It does not use a fixed
project path or silently delete Active/Protected. For external outputs, the
owner must name the exact path, establish that it is rebuildable, authorize
cleanup, and record the result separately. Never preserve an old report by
renaming it as a new record, and never make a new record by editing an old
hash or receipt.

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
