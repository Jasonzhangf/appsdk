---
name: appsdk-project-governance
description: "AppSDK quality gates and defect tracking; keep optional coordination separate from quality admission."
---

# AppSDK Project Governance

## Purpose and mandatory boundary

AppSDK verifies engineering quality. Collab supports automatic multi-worker
registration, communication and task/file ownership. Memory and Guidance help
when useful. Missing auxiliary state does not fail independent development.

Default flow: understand goal/scope → implement → relevant verification →
review → authorized delivery. Require applicable quality, safety and evidence
integrity gates; do not turn every available command into a mandatory phase.
- External AppSDK: compiler, CLI, schemas, harness, adapters, immutable rules.
- `.appsdk/`: committed project governance contract, maps, goal, records, verification, and `sdk.lock`.
- `.appsdk-control/`: ignored local run state, review cache, temporary harness output, and worker state.
- `playground/`: mutable experiment source.
- `active/lib/`: immutable consumable library.
- `protected/`: frozen source, contracts, and history.
- `generated/` or the project-declared artifact root: compiler output only; never hand-edit.
- AppSDK communication control: `appsdk::communication` owns the stable `appsdk-comm/v1`
  request/event/capabilities contracts and the replayed notification/Loop projections.
  Keep `.appsdk-control/communication/mailbox.jsonl` local and ignored; host integrations
  select a registered adapter (`mailbox` or `appserver`) instead of copying transport
  logic into a project. An App Server adapter must bind its target to a registered
  recipient and declare `send_message_to_thread`. Detailed route, batching, wakeup,
  receipt, and Bug/Loop gate semantics live in
 [`docs/design/apps-sdk-communication.md`](../../docs/design/apps-sdk-communication.md).

Run project commands from project cwd. An explicit optional project path is for
operators intentionally working elsewhere; no project-root environment variable.

## Truth and path ownership

The host-wide persistent truths are fixed and must not be inferred from a
project directory:

```text
~/.appsdk/{projects,runtimes,communication}.jsonl   AppSDK host truth
~/.collab/{server.sock,events.jsonl,log.txt,routes.jsonl,...}
                                                   Collab host truth
```

Project-local state has a different scope and owner:

```text
<project>/.appsdk/          AppSDK project contract, maps, records and lock
<project>/.appsdk-control/  AppSDK-owned local run/cache state
<project>/.agent-collab/    Collab-owned project registration/reducer input
```

None of the project-local paths is the global truth, and none is proof that the
current peer is registered. Read live registration, role, liveness and peers
through `collab context`; retire project-local state only through the owner's
canonical reset/migration command. Do not inspect or edit `~/.appsdk`,
`~/.collab`, `.appsdk-control/`, or `.agent-collab/` to reconstruct control
state.

The current client is Codex only. A peer is bound to the Codex sessionID
through the live App Server thread; the global Collab store is the identity,
route, mailbox, task, and liveness truth. Tracked `.appsdk/` files are present
in a Git worktree because they are committed, but ignored `.agent-collab/` and
`.appsdk-control/` state is not inherited. Ordinary project initialization and
Collab registration run from the canonical project main checkout. An
authorized AppSDK `--fresh --discard-legacy` reset is a separate operation and
may run from its clean non-main owner worktree as specified below. Inside a
worktree the same Codex sessionID/thread remains the same peer; return to the
canonical project main checkout for route recovery or master promotion. Never
register the worktree as a second peer or promote yourself from a worktree.

## SDK source repository and managed project boundary

This Skill is used in two different contexts and must not blur them:

- **AppSDK source repository:** a checkout containing the SDK implementation,
  release scripts, contracts, docs, and Skills (for example, `rust/` and
  `scripts/install-global-appsdk.sh`). Its root is an SDK development and
  release surface by default. A missing `.appsdk/project.json` means that the
  checkout is not implicitly a managed consumer project; it does not make
  initialization impossible. If the user explicitly chooses to govern this
  SDK workspace with AppSDK, run `appsdk prepare` and confirm the preparation,
  then run ordinary `appsdk init` from the canonical project main checkout.
  The normal initialization path creates a project contract that the owner
  must review and bind to the SDK source modules; it does not infer or
  overwrite those modules. A `playground/<slug>` worktree used to develop the
  SDK remains an SDK source worktree unless that explicit project registration
  is made. Its
  source, Git history, and release gates remain SDK-owned. Never run
  `appsdk init` or `appsdk reset-governance` merely to manufacture a contract,
  and never use fresh reset without the existing contract and explicit
  `--fresh --discard-legacy` authorization. Any local `.appsdk-control/` state
  is inspected as local runtime state and is not a reason to delete source
  repository files.
- **AppSDK-managed business project:** a consumer root with an explicit
  `.appsdk/project.json` and its project-owned goal, maps, records, module
  contracts, and `sdk.lock`. `appsdk prepare`, `init`, `verify`, `compile`,
  promotion, freeze, and an authorized governance reset operate on this root.
  A source repository or an arbitrary `cwd` is never treated as a business
  project without that contract. To govern a child project inside a larger
  checkout, first name that relative root through the preparation flow and
  bind it in the resulting contract.

The SDK source repository can still use an explicitly enabled Collab route for
its own TUI development, but that route proves agent communication only; it
does not by itself create a project contract or authorize a governance reset.
An explicit, confirmed `appsdk init` is the separate opt-in that registers the
SDK workspace as a project. Conversely, initializing a managed business project
does not grant authority over the SDK source repository. Keep source/release
evidence, project governance truth, and Collab runtime state in their
respective owners.

## One global AppSDK binary

Do not copy or select AppSDK binaries by hand. The AppSDK repository's only
supported global installation entry is:

```bash
scripts/install-global-appsdk.sh
```

It builds the release, atomically replaces the executable beside the active
`cargo`, removes exact AppSDK-managed legacy copies, and checks that one
managed `appsdk` remains. The same release source installs
`appsdk-project-governance`, `appsdk-migration`, and `project-memory` under
`~/.agents/skills/`. Run it from any directory; it resolves its own repository
root. SHA-256 is diagnostic output only, not a fixed admission condition. Do
not stop project development because a historical binary hash differs. If the
version or command path is wrong, run the installer once and refresh the
current shell cache (`rehash` in zsh or `hash -r` in bash); do not manually
copy, rename, or leave `.local/lib/appsdk/<version>/appsdk` beside the
canonical entry.

An AppSDK binary install does not restart a daemon. Use the daemon's official
maintenance command separately when the running process must load the new
binary. Never start v2 or create a second global AppSDK entry as a workaround.

## Legacy governance inventory and reset boundary

The canonical inspect, snapshot, freeze, reset or migrate, identity rebind,
restart, and verify state machine belongs to the
[AppSDK migration Skill](../appsdk-migration/SKILL.md). This project Skill only
defines what a managed project may classify, preserve, and hand to that Skill;
do not copy the migration state machine into this file or into a project.

For an existing project that contains legacy `.appsdk/` or `.agent-collab/`,
start with the exact operator path in
[Existing project: remove old governance](references/bootstrap-migration.md#existing-project-remove-old-governance).
The AppSDK reset and the Collab migration are separate owners and separate
transactions. Never treat removal of `.appsdk/` as permission to delete or
rebuild `.agent-collab/`, and never use the Collab migration as a substitute
for an authorized AppSDK governance reset.

The only clean-epoch reset entries are:

```sh
# AppSDK-owned project control plane; requires an existing contract and a
# clean non-main owner worktree.
appsdk init <project> --fresh --discard-legacy
# Same transactional reset owner, lower-level entry:
appsdk reset-governance <project> --discard-legacy

# Collab-owned project control plane; explicit authorization and a controlled
# daemon maintenance window are required.
collab down
collab reset --discard-legacy --approval "<explicit user authorization>"
collab up
collab init
```

`collab reset` archives the exact `.agent-collab/` and `.agent-collab-v2/`
bytes, removes only Collab-owned control state and stale routes, and records
`delivery_verified: false`. It never removes `.appsdk/` or
`.appsdk-control/`. Never manually delete either project-local root, journal,
mailbox, identity, task record, or route.

Before choosing a route, record an inventory of every exact path and runtime
object in the run note. At minimum include the AppSDK contract root and its
records/maps, `.appsdk-control/`, declared generated roots, Active/Protected,
business source/runtime data, every Collab initialization root, daemon
PID/socket, identity and route binding, mailbox/journal, claims, tasks, and
worktrees. For each item record its owner, observed status, content or
identity digest when applicable, retention class, proposed disposition, and
the evidence that makes the classification trustworthy. A filename, stale
screen, or successful daemon status is not an inventory decision.

Choose exactly one of these routes for a managed business project:

- **Preserve and migrate:** retain immutable project evidence, resolve one
  owner for ambiguous state, and invoke the canonical migration Skill. Old
  PASS, hashes, receipts, and review claims are historical witnesses; they are
  never copied into a new record or treated as proof for the new binary.
- **Reset and reinitialize:** only after the user authorizes discarding the
  named legacy control plane, from a clean non-`main` owner worktree with no
  competing claim. For an existing project that must start a new governance
  epoch, use the single explicit entry
  `appsdk init <project> --fresh --discard-legacy`; it performs the canonical
  reset and current-contract rebuild together, and records `mode: "fresh_init"`.
  The current SDK scaffold is the only reset baseline. Legacy SDK pins, migration
  witnesses, record/transition contracts, indexes, and rebuildable projections
  are ignored and regenerated from that baseline; missing SDK-owned fields are
  refilled. Only project-owned identity, module ownership, build declarations,
  and protection boundaries are carried forward. It must not replace those
  boundaries with the generic `change-me/app-core` scaffold. After the old
  control plane is removed, validation runs only against the new staging
  baseline. The lower-level `appsdk reset-governance --discard-legacy` uses the
  same transactional reset owner. Neither route inherits delivery, review,
  freeze, or deployment claims.

Reset may remove the old `.appsdk/` records/transactions and declared
rebuildable generated projections, plus local `.appsdk-control/` state owned by
that managed project. It preserves business source, runtime data, `active/`,
and `protected/` by default. Failed staging belonging to a live task must go
through that task's retry/abort owner first. `dist/`, `.deploy/`, `build/`,
`tmp/`, custom reports, vendor outputs, and other external paths require an
exact-path rebuildability decision and a separate authorization/cleanup
record.

`.agent-collab/`, its journal/mailbox, identity tokens, daemon PID/socket,
claims, task records, and worktrees remain Collab-owned. This Skill never
deletes or hand-edits them to make a migration appear clean; use the Collab
migration/recovery contract and preserve its evidence. After either route,
report retained and removed classes separately and verify one current truth.
A clean directory is not evidence of delivery, review, install, restart, or
live communication.

## Working loop

1. Read project AGENTS and affected code/contracts. Resolve owner, scope,
   acceptance and relevant gates. Read historical notes only when they help.
2. Use a clean owner worktree from latest origin/main. Preserve others' work.
   For multi-worker work, automatically register the peer, communications and
   task/file scope through official Collab; enforce overlap/resource ownership.
3. Implement the smallest adequate change. Use existing design for local work;
   clarify only material unknowns. Do not require a new plan or approval when
   scope is already authorized and clear.
4. Run applicable tests, necessary build and actual entrypoint checks. Install
   and restart only when required by the delivery object. Fix failures at their
   owner, never forge evidence or hide errors.
5. Review exact validated changes under the shared review standard. Block
   concrete correctness, safety, contract or material structural regressions;
   optional simplifications are advisory.
6. Treat each stage as a re-entrant gate. Persist candidate/tree, scope,
   dependency, artifact, producer and environment identity with its PASS
   evidence. Reuse only when that identity is unchanged and fresh; otherwise
   rerun from the first invalidated stage and its downstream dependants. A
   stage-name or session change alone never forces a full rerun. Report reused
   and rerun stages separately.
7. Deliver within authorization. Report test, review, merge, install, publish
   and resource cleanup as separate achieved states.

## Quick start: new governed project with Collab

For a new business project that also needs agent-to-agent Collab, do not invent
project-local transport or old `.appsdk/` state. Run the AppSDK flow from the
project root, then let `appsdk init` invoke official Collab once when a live
Codex App Server sessionID binding exists. App Server is the only supported
Collab transport.

```bash
cd /abs/path/project
appsdk prepare
appsdk init .
appsdk guide status
appsdk verify
```

`appsdk prepare` is mandatory for a new root. It writes
`.appsdk-prepare.json` as a draft, and `appsdk init` fails with
`PREPARATION_NOT_CONFIRMED` until the preparation fields are user-confirmed:
`status: "confirmed"`, `change_kind`, `project_root`, `boundary`, `questions`
closed, `confirmed_by`, and `confirmed_at`. Do not confirm scope on the
user's behalf and do not bypass prepare by editing `.appsdk/project.json`.
Detailed fields and an example are in
[bootstrap-migration.md](references/bootstrap-migration.md).

In a live Codex App Server runtime, `appsdk init` calls `collab init` once:
Collab starts/reuses its daemon, selects the App Server transport by server
capability, registers the current peer, and arms the default
`direct-message` lease. `collab init` resolves the project scope from the exact
process `cwd`. If no registered App Server route exists, AppSDK initialization
still succeeds for independent development, reports Collab pending, and never
fabricates a peer or notification channel. Then use `collab context`,
`collab sendmessage`, `collab inbox`, and `collab recv` only through the
server-selected transport.

For an already governed project, the initialization contract is only:

```text
collab context
-> registered: stop
-> unregistered or context fails before registration: appsdk init .
-> collab context
-> role=master requires user approval and no live master; otherwise remain peer
```

Do not pre-probe environment, panes, `routes.jsonl`, or `.agent-collab/`.
For the roles and copy/paste prompts after initialization, see
[bootstrap-migration.md](references/bootstrap-migration.md#master-and-ordinary-peer-bootstrap).
Master initialization adds `collab master promote --approval "<user text>"`
after the peer is live and the user explicitly approved the exact project and
peer; ordinary peers only verify identity, liveness, transport, presence and
task scope. Long-horizon goal scheduling is master-only and is verified with
`appsdk goal status --json` plus one real fired/consumed deadline replay, not
by command output alone.

## Quick start: replace old governance with current baseline

When a project already contains old `.appsdk/`, `.appsdk-control/`, or
`.agent-collab/`, treat AppSDK and Collab as separate owners with separate
transactions. First read
[Existing project: remove old governance](references/bootstrap-migration.md#existing-project-remove-old-governance)
and run the exact commands there. The invariant is:

```text
inventory both roots -> migrate/retire Collab first -> authorize AppSDK reset
-> clean non-main owner worktree -> appsdk init <project> --fresh --discard-legacy
-> appsdk guide compile -> appsdk verify
```

`appsdk init --fresh --discard-legacy` is the only init path that discards the
named AppSDK legacy control plane. It requires an existing
`.appsdk/project.json`, a clean non-`main`/`master` owner worktree, and explicit
authorization. It removes old AppSDK control state and declared generated
roots, refills missing current SDK fields, carries forward project-owned
boundaries, and never deletes `.agent-collab/` or old Collab evidence. Collab
state is removed or migrated through the `collab migrate` and daemon lifecycle
owned by the Collab Skill. A fresh reset record proves reset only; it never
imports old PASS, review, install, restart, delivery, or live communication.

For the Collab half, use `collab migrate` when the journal is replayable. Use
the explicit `collab reset --discard-legacy --approval "<user text>"` path only
when the operator authorizes abandoning the old Collab epoch. The two reset
commands are independent; neither one can claim the other's cleanup or
delivery result.

### Upstream AppSDK defect report

When the defect belongs to AppSDK itself, query for an existing report, file
one upstream record with reproduction and runtime identity, then read the
created record back:

```bash
appsdk bug list -q "<symptom>" --json --upstream
appsdk bug new --upstream -t "[SDK Bug] <symptom>" \
  -m "<reproduction, expected, observed, version, commit, logs>" \
  -l "P0,appsdk"
appsdk bug show <id> --json --upstream
```

The report is evidence of a filed defect, not proof that the local delivery or
the upstream fix passed.

## Conditional delivery gates

Candidate, review, integration, publication and runtime replay are separate
evidence states. Run only the checks and service operations declared by the
changed module and requested delivery. Ordinary development or documentation
work stops after its applicable checks and review; it does not acquire a
freeze, install, restart, live replay or full-suite ceremony by default.

Use [review-delivery.md](references/review-delivery.md) for the selected
delivery path. It binds every phase to the exact candidate, artifact,
environment and producer identity. Reuse unchanged PASS evidence after a
lightweight integrity/freshness check; do not rerun the external test,
deployment, merge or publication action merely because a later phase started.
If a required input changes, invalidate only that phase and its downstream
dependants. A candidate, review PASS, merge, push, install, restart or cleanup
receipt never implies any other state.

## Optional Guidance

Use `appsdk guide status/init/plan/update/next/close` when the user/project
selects persistent planning or a long task benefits from recovery. Default
`advisory` and `warning` do not require a task plan or setup before development.
Missing PlanRecord does not fail ordinary `verify` or `compile`.

When using Guidance, follow its declared transitions and bind observations to
the current context. A failed optional workflow is not a failed quality gate.
Do not fabricate a successful step to close a plan.

For a requested setup/upgrade, `guide init --mode bootstrap` is read-only.
Compare current project-owned sources with the advisory standard template;
apply only authorized rule changes. Ordinary `appsdk init` refreshes SDK
resources but never overwrites project AGENTS, Skills, records, Active or
Protected. The explicit `appsdk init --fresh --discard-legacy` route is the
user-authorized exception: it removes only the named legacy control plane and
rebuilds current SDK-managed contracts from the current scaffold baseline:
legacy SDK pins, migration witnesses, indexes, and rebuildable projections are
ignored, missing SDK-owned fields are refilled, and project-owned identity,
module ownership, build, and protection boundaries are carried forward.
Business source, runtime, Active and Protected are preserved. Merely auditing
rules does not require running initialization or changing setup.

## Optional Collab coordination

Collab is a coordination adapter, not a quality-admission prerequisite. The
AppSDK source repository and a managed consumer project keep separate owners;
initializing one never grants authority over the other. A missing App Server
peer, daemon, mailbox or native task route leaves independent AppSDK work
runnable; only an operation that explicitly needs shared ownership or
communication waits, with the exact Collab error preserved.

When managed child coordination is selected, use the canonical **subworker**
term and the `appsdk subworker` compatibility entry documented in the
[subworker policy](references/subagents-config.md). That entry forwards to
Collab and never creates a second registry, native Desktop thread or quality
gate. For identity, scope, route and two-way delivery evidence, follow the
Collab Skill and its live-route contract; do not duplicate that state machine in
AppSDK governance. Desktop does not register or subscribe a long-horizon goal.

## Universal Bug Tracking & Defect Governance

Execution-bound user inputs, requirements, problems, defects, and features use
one development intake backed by the existing `git-bug` store:

```bash
appsdk bug intake --input <intake.json>
```

The JSON declares `execution_bound: true`, classification `bug` or `feature`,
title, original input, scope, owner, optional parent, acceptance, status,
evidence links, and a dedup query. Intake queries first, reuses an exact
match, appends changed intake details, reopens a closed match, or creates one record.
It returns the authoritative `issue_id`. Read-only conversation uses no intake
and `execution_bound: false` is rejected.

Master, peer/worker, and subworker prompts use this same contract. Bind the
returned ID through worktree, implementation, tests, review, merge, and
closure. Without an ID, do not claim governed completion. Do not add another
issue database, scheduler, daemon, or task truth.

`WorktreeRecord` retains `bug_triage` and its query binding for non-legacy IDs.
Closing or promotion still requires canonical solution evidence:

```bash
appsdk bug close <id> -m "Solution: <root cause and resolution>" --receipt-id <receipt>
```

Legacy empty, `none`, and `legacy-*` IDs remain exempt from retroactive intake.
AppSDK framework defects retain the explicit `--upstream` route. Blocked tasks
still require cause, owner, unblock condition, and recovery trigger.

### Stage gates: re-entry and reuse

Treat each lifecycle phase as its own persisted gate. The phase projection is
bound to the candidate/tree, module scope, dependencies, artifact and
environment, map hashes, evidence IDs, and phase-specific mainline or cleanup
identity. On a new invocation, validate the current projection and upstream
records before doing work.

- A matching PASS projection with unexpired evidence returns `reused: true`
  and skips the external action that produced it. Keep the lightweight
  integrity, identity, and freshness checks; `reused` is not fresh test,
  deployment, merge, or publication evidence.
- If any bound input drifts, the current phase and its downstream phases are
  stale. Keep the immutable PASS record and produce a new candidate-bound
  projection; do not rewrite or downgrade the old record.
- `fail`, `unknown`, malformed, and expired records never count as PASS. The
  same non-PASS identity returns `LIFECYCLE_CHAIN_STAGE_NOT_PASS`; a changed
  identity archives the prior projection and re-enters the phase. Attempt
  history is append-only at
  `.appsdk/records/attempts/<module>/<phase>.jsonl` and is itself validated.
- `produce-lifecycle-records` reuses the Worktree/Reproduction/baseline set
  only when all three records and the complete declaration match. A partial
  set or drift is an explicit failure; never fill a missing record from a
  guessed cache. `verify` may reread the full graph for integrity without
  rerunning external commands.

This staged reuse is part of AppSDK quality governance and has no dependency on
Collab, Codex TUI, Desktop, or a particular agent runtime.

## Long-Horizon Goal Subscription & Master Saturation

`collab context` returns identity, liveness, tasks, inbox, `next_actions`,
master/authority state, and truth. The active `role_brief` comes only from the
registration receipt. Treat the current context plus role brief as the
contract. Master dispatches rather than codes: split and assign work, allocate
resources, keep workers loaded, own blockers, and drive
verify/merge/cleanup/close.
Independent worker owns its task end to end and evaluates master collaboration
requests against current ownership/capacity—accept non-conflicting work or
negotiate explicitly. Managed subworker executes its assigned scope and reports
evidence to parent/master. On trouble, worker/subworker first investigates, then
reports root cause, attempts, proposed fix, and exact decision needed.

Notifications are interrupts, not completion. Follow the `P0/P1/P2 ACTION`,
then resume current work; with no task, run `appsdk longhorizon show`. Never end
on ACK, read, or summary.

For a live peer, `collab context` is the authority and task-state query; use
the registration receipt for `role_brief` and do not use `whoami` as a second
initialization path. When no work is owned, run
`appsdk longhorizon show --json`. Long waits must use the supported timer/wake
path and then stop; do not poll in a loop.

Register complex or long-running goals only after the plan file exists and the
master has verified its live role. The goal is a one-shot deadline that must be
rearmed explicitly:

```bash
appsdk goal subscribe --goal docs/goals/<feature>-plan.md --interval 10m
```
- Path must point to an existing markdown file (`.md`).
- A goal prompt is only an execution pointer to that plan; it does not contain
  a second plan or register itself. Follow
  [goal-prompt.md](references/goal-prompt.md) and emit the prompt only after
  the plan exists and the goal is confirmed/admitted.
- For an MVP→M1 migration or closeout, the referenced plan must bind the MVP
  baseline, M1 target, owner/scope, legacy inventory and authorized route,
  identity/route proof, the Loop's Trigger/Work/Gate/State/Stop components,
  exact positive/negative gates, and post-merge/install/restart replay. The
  canonical migration state machine remains in the
  [AppSDK migration Skill](../appsdk-migration/SKILL.md).
- Desktop must not call `appsdk goal subscribe`. Goal registration belongs to
  the authorized live TUI/master endpoint; a prompt, appserver status, or
  daemon health cannot substitute for that authority.
- Master is awakened periodically to:
  1. Inspect worker states with `collab context` and `appsdk subworker status`; dispatch decomposed tasks to keep workers saturated whenever any worker is idle.
  2. Enforce AppSDK lifecycle governance across all subworker tasks.
  3. Report any upstream AppSDK framework issues via `appsdk bug new --upstream`.
  4. Conclude only when all goal DoD conditions pass.

The master's primary responsibilities are task decomposition, resource
allocation and recovery, worker saturation, blocker ownership, independent
review routing, merge/integration, bug management, final acceptance, and
cleanup. The master owns the P0/P1 queue and dirty `main`: triage and dispatch
the highest-priority open bugs, resolve or explicitly contain `main` dirt
before integration, and do not leave either queue waiting for a worker to
volunteer. The master does not write ordinary product code; implementation
belongs to the task owner. The master keeps architecture, integration and
critical repair only. Every assignment must state done-iff, allowed and
forbidden paths, worktree/branch, exact test commands, expected result, and
evidence location.

## Evidence and state ownership

- Project AGENTS owns project facts; Skills own procedure; declared machine
  contracts own enforceable gates. Existing lifecycle records remain the sole
  evidence truth. Plans, notes and Collab statuses do not duplicate PASS.
- Runtime review admission retains whitebox, public-entrypoint blackbox and
  exact candidate/artifact/environment identity. Module `deployment_operations`
  declares required `install`/`restart` receipts; omission retains both for
  compatibility, `[]` means neither operation applies. Bind this choice before
  validation; changes invalidate artifact identity. Every supplied receipt is
  checked. A missing required capability remains a blocker.
- Review confidence scores are optional annotation, never proof of quality.
- Freeze/Active/Protected apply when immutable artifact publication is in
  scope. Do not require freezing for a documentation edit or ordinary review.
- Engineering delivery may complete with a retained worktree. Keep ownership
  and cleanup obligations explicit; only claim resource closure after actual
  safe cleanup. No forced deletion to make a task appear complete.
- Memory is optional. No automatic durable memory/rule promotion. Long tasks
  and handoffs may record concise decisions and references to existing evidence.
  Memory migration and re-entry are explicit independent operations: use
  `project-memory migrate` for a source-preserving, resumable schema move and
  `project-memory index|export` to render old and current raw records as a
  Markdown index/details directory; after an intentional detail edit, use
  `project-memory import` to append the change back to raw history. Markdown
  is an interchange view, not a second truth store.
  Normal memory writes use one `project-memory entry` invocation, which writes
  the raw event and regenerates detail/index/projection together; do not hand
  write one of those derived files as a separate step.
  `memory/index.md` contains fixed-size Skill description candidates. Their L2/L3
  lines already include the kind, tags, and relative `L2/` or `L3/` detail path.
  During initialization or an intentional refresh, manually carry deduplicated
  L1 lines into the project Skill description, then fill unused slots with L2
  and L3 lines. Memory writes never rewrite Skill descriptions automatically.
  `project-memory reentry [project] --run <run-id>` to resume the same run after
  interruption. A missing or rebuilding memory index is not a governance
  failure, and memory state must not be reconstructed from Guide, debug,
  develop, or log payloads.

## References: load only the relevant domain

- Initialization or migration: [bootstrap-migration.md](references/bootstrap-migration.md).
- Development/debug: [development-debug.md](references/development-debug.md).
- Runtime review/delivery/freeze: [review-delivery.md](references/review-delivery.md).
- Selected persistent planning: [process-control-harness.md](references/process-control-harness.md).
- Contract errors/compatibility: [contracts-and-failures.md](references/contracts-and-failures.md).
- Explicit goal-prompt request: [goal-prompt.md](references/goal-prompt.md).

Failure reports name the failed applicable gate, preserved state, owner and next
action. Never infer deployed success, merge, freeze or cleanup from an earlier
test or an auxiliary workflow close.
