---
name: appsdk-project-governance
description: >
  AppSDK engineering quality gates and Universal Bug Tracking
  (appsdk bug new/list/show/comment/close): all user inputs (features &
  defects) are tracked as bugs. Master triages via `appsdk bug list -q`,
  reopens or creates `appsdk bug new -t -m -l "P0,mod"`, manages Kanban
  priorities, and dispatches workers. Worker inspects `appsdk bug show`, stays
  focused, reports new discoveries via `appsdk bug new` without auto-fixing,
  reports blockers to master, and closes with `appsdk bug close ID -m
  "Solution: ..."`. Dependency managed via `appsdk setup-deps [--check]`. Use
  optional Guidance for planning; keep automatic Collab separate from quality
  admission.
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
  select a registered adapter (mailbox, tmux, or appserver) instead of copying transport
  logic into a project. tmux/appserver adapters must bind their target to a registered
 recipient. Detailed route, batching, wakeup, receipt, and Bug/Loop gate semantics live in
 [`docs/design/apps-sdk-communication.md`](../../docs/design/apps-sdk-communication.md).

Run project commands from project cwd. An explicit optional project path is for
operators intentionally working elsewhere; no project-root environment variable.

## SDK source repository and managed project boundary

This Skill is used in two different contexts and must not blur them:

- **AppSDK source repository:** a checkout containing the SDK implementation,
  release scripts, contracts, docs, and Skills (for example, `rust/` and
  `scripts/install-global-appsdk.sh`). Its root is an SDK development and
  release surface. A missing `.appsdk/project.json` at that root is expected;
  do not run `appsdk init` or `appsdk reset-governance` there merely to make the
  SDK repository look like a consumer project. A `playground/<slug>` worktree
  used to develop the SDK remains an SDK source worktree. Its source, Git
  history, and release gates are governed as SDK work; any local
  `.appsdk-control/` state is inspected as local runtime state and is not a
  reason to delete the source repository's files.
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
does not create an AppSDK consumer contract or authorize a governance reset.
Conversely, initializing a managed business project does not grant authority
over the SDK source repository. Keep source/release evidence, project
governance truth, and Collab runtime state in their respective owners.

## One global AppSDK binary

Do not copy or select AppSDK binaries by hand. The AppSDK repository's only
supported global installation entry is:

```bash
scripts/install-global-appsdk.sh
```

It builds the release, atomically replaces the executable beside the active
`cargo`, removes exact AppSDK-managed legacy copies, and checks that one
managed `appsdk` remains. Run it from any directory; it resolves its own
repository root. SHA-256 is diagnostic output only, not a fixed admission
condition. Do not stop project development because a historical binary hash
differs. If the version or command path is wrong, run the installer once and
refresh the current shell cache (`rehash` in zsh or `hash -r` in bash); do not
manually copy, rename, or leave `.local/lib/appsdk/<version>/appsdk` beside the
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
  The lower-level `appsdk reset-governance --discard-legacy` remains available
  for its existing idempotent reset route. Neither route inherits delivery,
  review, freeze, or deployment claims.

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
6. Reuse still-valid evidence when relevant source, inputs, dependencies,
   configuration, artifact and environment remain unchanged. Rerun affected
   checks after changes; verify an altered integration candidate.
7. Deliver within authorization. Report test, review, merge, install, publish
   and resource cleanup as separate achieved states.

## Mainline delivery gate

Every AppSDK or runtime change uses this order:

```text
clean playground worktree
  -> candidate tests/build
  -> independent review
  -> clean main integration
  -> main tests/build
  -> push + remote receipt
  -> official global install
  -> exact service-scoped restart
  -> deployed public-entrypoint replay
  -> task/worktree cleanup and close
```

These are separate evidence states. A candidate, review PASS, local merge,
remote push, installed binary, daemon restart, live replay, or cleanup receipt
does not imply any other state. Never install or restart from a worker branch.

### Required delivery procedure

1. Create `playground/<slug>` from current `origin/main`. The worker never
   edits `main` or shares a worktree.
2. Run focused tests, relevant full suite, formatter, diff check, and release
   build. Record the candidate commit, tree, artifact, and exact commands.
3. Use an independent review and replay the unchanged-source effectiveness
   case. Review PASS is required before integration.
4. Integrate only into a clean local `main`. If tracked or untracked user
   changes exist, stop and report exact paths; never reset, restore, stash,
   overwrite, or silently absorb them.
5. Re-run affected tests, full tests, formatter, diff check, and release build
   on the exact merged commit. Record the merge commit and artifact hash.
6. Push only that tested main commit. Verify remote truth with
   `git ls-remote <remote> <ref>`; a local tracking ref is not publication.
7. Install through the canonical AppSDK entry point:

   ```bash
   scripts/install-global-appsdk.sh
   appsdk version
   shasum -a 256 "$(command -v appsdk)"
   ```

   Preserve source commit, installed path, version, and digest. Do not copy a
   binary by hand or leave a second global SDK entry.
8. Restart only affected services with their official service-scoped command.
   For Collab, use exactly:

   ```bash
   env -u TMUX_PANE collab down
   env -u TMUX_PANE collab up
   ```

   Run this once per exact project root. Record old/new PID, socket, binary
   path, and digest. Never use `pkill`, `killall`, `xargs kill`, or broad PID
   commands.
9. Run the deployed public-entrypoint replay and negative path. For Collab,
   verify `collab who`, `collab context`, `collab task status`, `collab inbox`,
   one real notification/consume path, and single-daemon identity after
   restart. Source tests are not live replay evidence.
10. Only after remote receipt, install/restart, and replay pass may the owner
    release its claim and clean its own worktree through the official task
    close operation. Cleanup records the removed path and receipt; it never
    deletes another task's worktree, mailbox, journal, or token.

The documented command surface must match the installed Collab binary. Before
using an optional lifecycle subcommand, run `collab task --help`. If the
installed version has no dedicated review or integration command, record the
same evidence with the supported `collab task update --status ... --next ...`,
`collab task deliver`, bug comments, and the mainline/remote receipts; never
invent a successful command or claim that an unavailable subcommand ran.

If a gate fails, preserve the exact error and leave the task explicitly
blocked with owner, unblock condition, next check, and recovery trigger. Never
report deployed from a candidate branch, merged from a local-only ref, or
complete from tests without mainline, install, restart, and replay evidence.

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
rebuilds current SDK-managed contracts; it still preserves business source,
runtime, Active and Protected. Merely auditing rules does not require running
initialization or changing setup.

## Automatic Collab

Persistent subagents and project-specific notification policy:
[subagents-config.md](references/subagents-config.md). All policies live in
`~/.appsdk/config.toml`; `appsdk config` shows effective configuration.
`appsdk subagent start/list/status/send/close` delegates to the Collab owner.

`appsdk init` attempts official `collab init` once in a live tmux peer, preserving
the inherited environment. Successful initialization registers identity and the
finite direct-message subscription. Do not duplicate that initialization.
Once task/worktree scope is known, use the Collab task lifecycle to register
feature/resource and file ownership before concurrent edits. Do not invent
task scope inside AppSDK initialization.

No tmux peer means pending. An unavailable/failed Collab reports its error;
independent work continues, while operations requiring shared ownership wait.
Keep automatic communication and file/task collaboration enabled; a serial
merge queue is required only when the project selects that integration mode.
Its ownership and tested-integration protections remain mandatory.

### SDK-source Codex TUI identity and route proof

The AppSDK source repository may opt into Collab for its own TUI development,
but source-repository communication and AppSDK consumer-project governance are
separate facts. In the source repository, use the standalone official Collab
registration once from the live Codex TUI pane when that route is explicitly
enabled; do not run `appsdk init` merely to manufacture a consumer contract.
For a managed project, the one bootstrap path remains `appsdk init .`. Never
run either initialization twice for the same live endpoint.

Identity is accepted only when the same live pane proves all of the following:

1. The pane and process are live, owned by the expected TUI, and their `cwd`
   is the intended SDK worktree or managed project root. A process listing,
   screenshot, `TMUX_PANE` value copied from another pane, or a shared directory
   alone is diagnostic evidence, not registration. Project scope is derived
   from that live `cwd`; an appserver or session name cannot override it.
2. `collab context` exposes the registered address and authority: `scopeId`,
   stable `sessionId`, `appserverId`/namespace when supplied, project root,
   role, `identity_valid`, and `endpoint_live`. `collab who` must show the
   expected registered peer(s). `collab status --all` proves daemon health only;
   it does not prove identity or a route.
3. The global Collab daemon has one authoritative PID/socket, and the route
   points to the same registered endpoint. A per-project or second daemon is
   not a recovery path. `PROJECT_SCOPE_UNKNOWN`, timeout, `identity-mismatch`,
   absent endpoint, or an unbound pane is a failed identity gate and must remain
   visible.
4. A two-way replay completes with separate evidence: the sender's durable
   message/notification ID and `accepted` result, delivery/consume evidence in
   the receiving TUI (`collab recv`), and a reply that the sender consumes.
   Do not call mailbox append, tmux preview, `accepted`, ACK, or daemon health
   a bidirectional reply by itself. Use a title and priority on each message.

The address is the registered `scopeId/sessionId`; compression, fork, model
name, or a changed pane does not silently preserve it. A new runtime address
must complete the official re-registration/rebind path and a fresh route proof.
Only an explicit user grant can make that registered peer a `master`; the SDK
repository, TUI root, first registration, and goal text cannot grant it.

## Universal Bug Tracking & Defect Governance

All user inputs—whether bug reports or new feature requests—are tracked through `appsdk bug` backed by `git-bug`.

### 1. Requirements Triage & Kanban Management
- **Master Role**:
  - Receives user inputs / feature requests / bug reports.
  - Queries existing issues first: `appsdk bug list -q "<keyword>" -l "<label>" --json`.
  - If an existing related issue is found, **reopen** it and append details.
  - If new, creates a new issue:
    ```bash
    appsdk bug new -t "<title>" -m "<requirements & reproduction>" -l "<priority>,<module>"
    ```
  - Prioritizes backlog using labels (e.g. `p0`, `p1`, `p2`) and dispatches workers based on highest priority issues within scope.
- **Worker / Subagent Role**:
  - Receives assigned issue and inspects its history: `appsdk bug show <id> --json`.
  - Verifies and reproduces the defect/feature in an isolated worktree.
  - Reports discoveries or new bugs to the bug system immediately; **does not auto-fix unrelated discoveries** to stay focused on the primary objective.
  - **Blocker Handling & Block Criteria**:
    - Task status can be marked as `blocked` (`collab task block <id>`) only with a concrete cause, responsible owner, unblock condition, and recovery trigger. Genuine external dependencies, resource ownership, missing credentials/approval, and cross-owner decisions may be valid waits; difficulty alone is not.
    - AppSDK framework defects remain an upstream bug path (`appsdk bug new --upstream -t "[SDK Bug] ..." -l "P0,cli"`), but non-framework failures must first be investigated and solved in scope. If a cross-owner decision is required, report a concrete proposal to Master; Master must take over, reassign, or auditable-force-close in the same cycle.
    - If encountering a valid AppSDK blocker and a live Master exists: report immediately to Master with root cause and proposed fix (`collab sendmessage --to <master> --subject blocker "..."`).
    - If blocked by AppSDK and no live Master exists: file an upstream SDK bug, resolve or work around, and resume the task.

### 2. Multi-Criteria Filtering
- Master and workers filter issues to reduce noise:
  - By status: `appsdk bug list --status <open|closed>`
  - By label: `appsdk bug list -l <labels>`
  - By participant/author: `appsdk bug list -p <user> -a <author>`
  - By keyword query: `appsdk bug list -q <query>`
  - By sort & direction: `appsdk bug list -b <creation|edit> -d <asc|desc>`

### 3. Lifecycle Evidence Enforcement
- **Architecture Gate**: `WorktreeRecord` must declare `bug_triage` (`query_executed: true`, a query containing the issue ID, `mode`, `reopened_from_issue_id`) verifying that existing issues were triaged before creating new work.
- **Promotion / Closure Gate**: Closing a bug or promoting a candidate requires solution documentation in `git-bug`:
  ```bash
  appsdk bug close <bug_id> -m "Solution: <root cause & resolution>" --receipt-id <receipt_id>
  ```
- **Legacy Compatibility**: Tasks with empty, `none`, or `legacy-*` `issue_id` are exempt from retroactive bug tracking enforcement.

### 4. Stage gates: re-entry and reuse

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
Collab, tmux, Codex TUI, Desktop, or a particular agent runtime.

## Long-Horizon Goal Subscription & Master Saturation

`collab init` / `whoami` returns `role_brief`; treat it as the active contract.
Master dispatches rather than codes: split and assign work, allocate resources,
keep workers loaded, own blockers, and drive verify/merge/cleanup/close.
Independent worker owns its task end to end and evaluates master collaboration
requests against current ownership/capacity—accept non-conflicting work or
negotiate explicitly. Managed subagent executes its assigned scope and reports
evidence to parent/master. On trouble, worker/subagent first investigates, then
reports root cause, attempts, proposed fix, and exact decision needed.

Notifications are interrupts, not completion. Follow the `P0/P1/P2 ACTION`,
then resume current work; with no task, run `appsdk longhorizon show`. Never end
on ACK, read, or summary.

Register complex or long-running goals with a required markdown target and periodic reminder interval:

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
  1. Inspect worker states (`collab who` / `appsdk subagent status`); dispatch decomposed tasks to keep workers saturated whenever any worker is idle.
  2. Enforce AppSDK lifecycle governance across all subagent tasks.
  3. Report any upstream AppSDK framework issues via `appsdk bug new --upstream`.
  4. Conclude only when all goal DoD conditions pass.

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
