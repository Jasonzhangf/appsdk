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
business files. In a live Codex App Server Agent it also invokes official
`collab init` once; Collab starts/reuses its daemon, evaluates the available
App Server capability for the current Codex sessionID, registers the current peer
through the server-selected transport, and arms the default finite
`direct-message` subscription. Do not run `collab init`, `collab whoami`, or a
manual ordinary-message subscription afterward. Use `new` only for an empty
destination.

`appsdk prepare` is a hard gate. First invocation writes `.appsdk-prepare.json`
with `status: "draft"`. `appsdk init` rejects an unconfirmed preparation with
`PREPARATION_NOT_CONFIRMED`; it does not guess scope or boundaries. Rerunning
`appsdk prepare` prints the existing record and does not overwrite it.

Before `appsdk init`, the operator must explicitly confirm these fields in
`.appsdk-prepare.json`:

```json
{
  "schema_version": 1,
  "preparation_id": "prepare-<slug>",
  "status": "confirmed",
  "objective": "One-sentence confirmed goal for AppSDK governance admission.",
  "change_kind": "new_project",
  "project_root": ".",
  "legacy_roots": [],
  "new_roots": [".appsdk/**", ".appsdk-control/**", "playground/**"],
  "protected_roots": ["protected/**", ".git/**"],
  "runtime_forbidden_roots": ["generated/**", ".agent-collab/**"],
  "boundary": {
    "allowed_paths": [".appsdk/**", "playground/**", "active/lib/**", "protected/**"],
    "forbidden_paths": [".git/**", ".agent-collab/**", "dist/**"],
    "payload_control_separation": "confirmed"
  },
  "acceptance_criteria": ["appsdk init completes", "appsdk verify passes"],
  "non_goals": [],
  "questions": [],
  "confirmed_by": "Jason (explicit user approval)",
  "confirmed_at": "2026-09-15T00:00:00Z"
}
```

`change_kind` must be one of `new_project`, `module_refactor`,
`project_refactor`, or `debug`. `project_root` is the relative AppSDK project
root from the preparation file location; `.` means the same directory. All open
questions must be answered or removed before `init`. Never confirm the record
on behalf of the user, and never replace the preparation gate by editing
`.appsdk/project.json` before initialization.

For an existing project with prior `.appsdk/` or `.agent-collab/`, do not use
this ordinary new-project path. Use
[Existing project: remove old governance](#existing-project-remove-old-governance)
and keep AppSDK and Collab reset/migration in separate transactions.

AppSDK preserves the launching environment and does not pass a project path to
Collab. `collab init` resolves project scope from the exact process cwd.
Without a live registered App Server transport, AppSDK initializes governance
and reports Collab pending because no peer can be registered; it never
fabricates subscription state.

Collab initialization errors are explicit warnings for AppSDK initialization.
Automatic multi-worker registration and task/file coordination remain enabled;
shared operations wait for reliable ownership while independent work continues.

### Master and ordinary peer bootstrap

For a project that will run multiple agents, initialize the AppSDK governance
root first, then register the current peer and explicitly assign the role.
Registration and wake bind the current Codex sessionID to the live App Server
native thread.

Master initialization, after user approval for the exact project and peer:

```bash
cd /abs/path/project
appsdk prepare
# confirm .appsdk-prepare.json exactly as above
appsdk init .
appsdk guide status
appsdk verify
collab context
collab status --all
collab who
collab master promote --approval "<user approval text>"
collab master status
```

Do not promote while a live master already exists. `appsdk init` alone never
creates master authority. If `appsdk init` reports Collab pending or a failed
registration, preserve the exact error; do not report Collab as available and
do not start a second daemon.

Ordinary peer initialization, after the project already has
`.appsdk/project.json` and a live master or parent:

```bash
cd /abs/path/project
appsdk init .
collab context
collab status --all
collab who
collab worker status <peer-id>
collab notify status
```

The peer must observe its own identity, liveness, presence, transport and
worker role. Do not run a second `collab init`, do not promote itself, do not
register a long-horizon goal, and do not fabricate a worker role from
`appsdk init` output.

Long-horizon master scheduling is a separate, master-only step. Create the
plan file first, then register and verify:

```bash
appsdk goal subscribe --goal docs/goals/<goal>-plan.md --interval 10m
appsdk goal status --json
appsdk longhorizon show --json
```

`appsdk goal status --json` must report `active: true`, `desired: subscribed`,
`observed: subscribed`, `collab_subscribed: true`, a non-null
`subscription_id`, and a null `error`. A successful command output is not
proof that a timer fired. Run one short-interval live replay and record the
armed subscription, fired deadline notification, and consumed result before
declaring long-horizon scheduling verified. The current implementation is a
one-shot deadline that must be explicitly rearmed with `appsdk goal subscribe`
after it is consumed, expires, or Collab restarts.

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
-> appsdk init <project> --fresh --discard-legacy
-> old .appsdk audit/migration records and generated projection removed
-> current .appsdk contract, record contracts, and transition manifest rebuilt
-> rebuild maps/goal/module/owner from current project truth
-> appsdk guide compile
-> appsdk guide init for the current task/domain
-> appsdk verify
```

`--fresh --discard-legacy` is an explicit existing-project initialization route,
not ordinary `init` behavior. It requires `.appsdk/project.json`, a clean
non-`main`/`master` worktree, and the discard confirmation. It preserves
business source, runtime data, `active/`, `protected/`, and human documents;
only the named AppSDK control plane and declared generated roots are removed.
The current SDK scaffold is the only reset baseline: legacy SDK pins, migration
witnesses, indexes, and SDK-owned contract projections are ignored and
regenerated; missing SDK-owned fields are refilled. Project identity, module
ownership, build, and protection boundaries are carried forward.
The new reset record has `mode: "fresh_init"` and proves the reset operation
only. It does not inherit old PASS, review, delivery, or freeze claims.

The lower-level `appsdk reset-governance --discard-legacy` command remains
available and uses the same transactional reset owner. Neither command
authorizes manual deletion or hand-editing of version/hash/ReviewRecord, and
neither permits two active governance roots. If old Active/Protected artifacts
are also obsolete, name exact paths and authorize a separate cleanup.

### What to do with old reports and delivery output

Use ownership and rebuildability, not age, to decide what is removable:

| Class | Default action | Reason |
| --- | --- | --- |
| `.appsdk/records`, `.appsdk/transactions`, audit/migration reports | Inventory/snapshot if needed, then remove through reset | Old control truth must not leak into the new baseline. |
| Declared `governance.generated_root`, module generated outputs | Remove through reset and regenerate | These are reproducible projections, not source or release truth. |
| Failed transaction staging | Canonical abort/retry if current; otherwise reset | Manual deletion can hide ownership or partial publication. |
| `active/`, `protected/`, runtime data, business source | Retain | They may be the only published or operational truth. |
| `dist/`, `.deploy/`, `build/`, `tmp/`, custom reports/artifacts | Keep until exact disposable ownership is confirmed | AppSDK cannot infer that an external output is safe to delete. |

The reset command reads and validates the old project contract before removal,
carries that contract into the new `.appsdk` root, and includes its declared
generated root in the disposable set. It does not use a fixed project path or
silently delete Active/Protected. For external outputs, the
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

## Existing project: remove old governance

Use this section when a real project root already contains `.appsdk/`,
`.appsdk-control/`, or `.agent-collab/` from an older version and the operator
wants to start the current governance and coordination baseline instead of
migrating old control state. The two roots have different owners and must be
handled in separate transactions.

### 1. Inventory and freeze

Run read-only inventory from the project root. Do not delete anything yet.

```bash
cd /abs/path/project
git status --short --branch
git worktree list
find .appsdk .appsdk-control .agent-collab -maxdepth 3 -print 2>/dev/null
collab migrate inspect
collab status --all
collab context
```

Record the exact project root, branch/HEAD, worktrees, `.appsdk/` and
`.appsdk-control/` contents, generated roots, `active/`, `protected/`,
`.agent-collab/` journal/mailbox/tasks/claims, daemon PID/socket, peer
identities, routes, and migration blockers in the run note. A file name, old
PID file, socket existence, or successful `collab status` is not sufficient
proof of ownership.

Stop new shared writes, dispatches, and task admission before either reset.
Preserve every active worktree and task until its owner or an explicitly
authorized migration decision resolves it.

### 2. Collab migration or retirement

`.agent-collab/` is owned by Collab. AppSDK reset does not remove it. If the
project uses Collab v1 and the journal is replayable, use the authenticated
migration transaction:

```bash
collab migrate inspect
collab migrate plan
collab migrate apply
# install the reviewed Collab binary, then:
collab down
collab up
collab worker recover
collab migrate verify
```

`inspect` is read-only. `plan` does not freeze admission. `apply` freezes
admission and persists the deterministic snapshot. `verify` resumes admission
only after journal/mailbox/task/identity continuity passes. If the result is
`reset_required`, `needs_operator`, `unknown`, or an owner/count mismatch, stop
and resolve it through Collab's canonical owner; do not continue to the AppSDK
reset as if the whole operation passed.

There is no supported project-level command that deletes and recreates
`.agent-collab/`. Never manually delete it, edit its JSON/JSONL, clear its
mailbox, copy identity tokens, or start a second daemon. A project that has no
valid Collab state to preserve still needs an explicit Collab retirement or
fresh-registration decision from the Collab owner; AppSDK must not make that
decision for it.

### 3. AppSDK reset

After the Collab side is either migrated/verified or explicitly retired by its
owner, handle `.appsdk/` from a clean non-`main` owner worktree with no
competing claim. For a project that must abandon the old governance epoch and
start from the current SDK baseline, the preferred single entry is:

```bash
cd /abs/path/project
git worktree add -b codex/governance-reset-<slug> \
  playground/governance-reset-<slug> origin/main
cd playground/governance-reset-<slug>
appsdk init "$PWD" --fresh --discard-legacy
appsdk guide init --task governance-reset --mode bootstrap --module <module-id>
appsdk guide compile
appsdk verify
appsdk compile
```

`appsdk init --fresh --discard-legacy` requires an existing
`.appsdk/project.json`, a clean non-`main`/`master` worktree, and the explicit
discard confirmation. It uses the same transactional reset owner as
`appsdk reset-governance --discard-legacy`, but combines reset with current
contract rebuild. It removes the old AppSDK control plane, `.appsdk-control/`,
and declared rebuildable generated roots; it preserves business source,
runtime data, `active/`, and `protected/` by default. It must not remove
`.agent-collab/`.

If the operator explicitly chooses the lower-level AppSDK operation instead,
run it in the same clean non-`main` worktree and then initialize:

```bash
appsdk reset-governance "$PWD" --discard-legacy
appsdk init "$PWD"
appsdk guide compile
appsdk verify
```

Do not hand-edit `.appsdk/` JSON, reuse old PASS/hash/receipts, or delete
`active/` or `protected/` as part of reset. Those are separate authorized
cleanup decisions.

If the old `.appsdk/project.json` is missing, malformed, unreadable, or the
worktree is dirty, `--fresh --discard-legacy` must fail closed. Do not replace
that check with manual deletion. Preserve the exact error and use the AppSDK
migration owner to establish whether the project contract can be recovered;
only an existing, valid contract can authorize a fresh reset. If no contract
can be established, a new root must go through a separate confirmed
preparation/init flow rather than claiming to reset the old project.

### 4. Verify the new baseline

The operation is complete only when all of the following are true:

- Collab reports the exact migration/retirement result and has one verified
  daemon/socket/identity state.
- `appsdk verify` passes against the current project contract and the fresh
  reset baseline.
- The removed classes are limited to the authorized AppSDK control plane,
  `.appsdk-control/`, and declared rebuildable generated roots.
- Business source, runtime data, `active/`, `protected/`, and all retained
  Collab evidence still exist.
- The new reset record is current and does not claim delivery, review,
  install, restart, or communication success.
