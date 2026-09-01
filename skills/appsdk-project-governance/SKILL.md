---
name: appsdk-project-governance
description: Integrate and operate AppSDK governance in a new project without copying the SDK implementation into the project. Use for project bootstrap, goal clarification, Playground changes, evidence/review/promotion, Active publishing, Protected freezing, SDK lock verification, and boundary audits.
---

# AppSDK Project Governance

Use the external AppSDK implementation as the governance engine. Keep only project contracts and lifecycle records in the target repository.

## Boundaries

- External AppSDK: compiler, CLI, schemas, harness, adapters, immutable rules.
- `.appsdk/`: committed project governance contract, maps, goal, records, verification, and `sdk.lock`.
- `.appsdk-control/`: ignored local run state, review cache, temporary harness output, and worker state.
- `playground/`: mutable experiment source.
- `active/lib/`: immutable consumable library.
- `protected/`: frozen source, contracts, and history.
- `generated/` or the project-declared artifact root: compiler output only; never hand-edit.

Never copy the AppSDK source, compiler, or harness into the business project. Never put committed project maps or lifecycle records in `.appsdk-control/`.

## Governance is part of the work

AppSDK governance is an execution responsibility, not an external
prerequisite. Missing governance, an unconfirmed goal, absent maps or records,
an invalid SDK lock, a dirty/shared worktree, an unavailable required adapter,
or any failed preflight is a project-management defect owned by the current
worker. Do not attribute it to an outside factor, skip the gate, weaken the
contract, or start business-code debugging around it.

When a preflight fails, stop code work, classify the first failing governance
node, repair the governance/management state through its canonical owner,
re-run the preflight, and only then reproduce or modify business code. The
closeout must record the failure, ownership, repair, and verification. A
missing governance record is not evidence that the business code is broken;
it is evidence that the lifecycle is incomplete.

Required order:

```text
governance preflight
  -> repair project-management gap
  -> verify preflight again
  -> reproduce/debug business code
```

This rule includes migration, worktree declaration, resource claims, maps,
evidence, review admission, build/install/restart, merge, and cleanup. A
tool/network/adapter failure may explain the symptom, but it does not remove
the worker's responsibility to manage the project state and resolve the
failure before code debugging continues.

## Collab lifecycle and main protection

AppSDK adopts Collab's independent-peer lifecycle. There is no master/worker
role and no central dispatcher. A worker owns one issue, one semantic claim,
one branch, and one clean worktree under the main worktree's
`playground/<issue-or-task>` directory. The main worktree is read-only for
development and lifecycle writes.

Required sequence for every debug or change:

```text
fetch latest origin/main
  -> create clean playground worktree and branch
  -> reproduce and record first divergence
  -> modify only in that worktree
  -> build and run whitebox checks
  -> commit and push candidate
  -> rebase/merge latest main into a fresh integration worktree
  -> verify the exact integration commit
  -> merge fast-forward to remote main
  -> verify remote main with git ls-remote
  -> close records, release claim, remove only the merged clean worktree/branch
```

Never edit, build generated state, pin, promote, freeze, publish, or migrate
from a checkout whose current branch is `main`; the CLI rejects lifecycle
mutations with `MAIN_WORKTREE_MUTATION_FORBIDDEN`. A detached or unnamed
checkout is invalid for the required lifecycle worktree and must be replaced
by a named feature branch worktree before mutation. `verify` remains read-only
and may inspect main. Do not use `git reset`, `git checkout`, `git restore`, or `git
stash` to make a dirty main usable. Preserve unrelated dirty files and return
the task to its owner worktree when a conflict exists.

## Forbidden versus warning

The following are hard forbidden gates: direct mutation of main; dirty or
shared candidate worktrees; missing owner/claim; source or record hash drift;
payload/control mixing; Protected/Active mutation; missing candidate or
integration identity; failed required tests; missing deployment/restart proof
when the project declares that gate; and cleanup before a verified remote-main
receipt. These fail closed.

The following are warnings during transition and must not block an otherwise
valid incremental change: legacy 0.1.5 records that were valid under their
original contract; retained historical strict gates not touched by the
change; missing optional progress/ACK/reporting metadata; an unneeded
parallel-collaboration scenario in a single-worker project; and ordinary
regression work that is not required by the changed module's map. Warnings are
recorded in the run evidence and must not be projected as PASS or used to
silently bypass a forbidden gate.

## Existing-version migration

An existing project may retain a valid old governance baseline, but upgrading
the control plane is an explicit, user-authorized transfer of ownership. The
old AppSDK governance state and each dispersed Collab initialization are two
independent historical truths. Never copy, concatenate, or infer one from the
other. First inspect both planes, freeze their mutations, and write immutable
snapshots with counts and hashes. Then request Jason's authorization to take
control of the exact project, task, resource, claim, worktree, daemon, and
staging objects named by the migration. No authorization means no takeover,
deletion, merge, or state reset.

Use this sequence for a 0.1.5 AppSDK project with dispersed Collab state:

```text
inspect AppSDK version/maps/records/Active/Protected/sdk.lock
  -> inspect every Collab root, identity, task/resource/claim, daemon and staging state
  -> freeze both control planes and snapshot each independently
  -> build an explicit AppSDK <-> Collab mapping
  -> classify exact matches, AppSDK-only, Collab-only, and conflicts
  -> design the complete compliant reconciliation route
  -> request approval for the explicit ownership-transfer or destructive step
  -> execute the approved route; keep only unapproved destructive steps frozen
  -> clean only authorized, proven-stale recoverable staging/runtime state
  -> install/pin the new AppSDK bundle and migrate Collab through its official v1 path
  -> verify one owner and one truth for every retained object
  -> reset only authorized runtime/temporary state to a clean initial state
  -> apply new rules to every new/changed node, then resume normal lifecycle
```

The mapping must preserve the source identity, object identity, owner,
references, and snapshot hash on both sides. Exact matches may be adopted;
AppSDK-only state receives a Collab binding; Collab-only state gets a proposed
legacy disposition with an explicit owner and mapping plan. Conflicts are not
merged and do not receive an invented owner: the worker must construct the
complete compliant reconciliation route, including the target truth, precise
object actions, collision handling, rollback boundary, and verification gates,
then request Jason's approval for the exact ownership transfer or destructive
step. This is an approval gate for execution, not a reason to merely report
and wait. Historical records and
immutable Active/Protected artifacts remain audit evidence. "清理历史包袱"
means removing only authorized stale staging, abandoned runtime leases,
duplicate queues, and other disposable control projections—not deleting
history to make `verify` green. After unified governance, the allowed reset is
an explicit zeroing of runtime/temporary state, not deletion of history or
referenced immutable artifacts.

For the AppSDK portion, execute the target 0.1.6 binary itself and pass that
exact file to `appsdk pin-lock` after authorization and snapshotting. Do not
hand-edit version, hash, ReviewRecord, migration, Active, Protected, mailbox,
identity, task, or claim files. A partial pin may resume only through its exact
migration record or exact source maps. Do not delete partial outputs by hand;
if cleanup is authorized, execute it through the canonical owner and retain
the cleanup receipt.
If old evidence is absent, a historical gate is a warning only when the module
is unchanged; changed/new modules must produce new evidence. Unsupported
versions, byte/source/map drift, hash mismatch, duplicate writers, ambiguous
mapping, or unauthorized cleanup/reset remain forbidden. The old valid state
is accepted as evidence until the authorized migration completes; it is not
silently reinterpreted under the new rules.

## Existing project bootstrap

For an existing project, begin with a preparation record instead of manually creating governance directories:

```bash
appsdk prepare <workspace>
```

The AI must read `.appsdk-prepare.json`, ask the user about change kind, project root, legacy roots, new roots, Protected roots, allowed paths, forbidden paths, and payload/control separation, then update the record to `status: confirmed`. Do not initialize before confirmation.

For an existing workspace, confirmed preparation must route through legacy
migration before `init` or business-code debugging:

```text
confirmed preparation
  -> inspect old AppSDK truth and every dispersed Collab root
  -> freeze and independently snapshot both planes
  -> design ownership-transfer/reconciliation route
  -> request approval for exact takeover/cleanup/reset operations
  -> execute approved migration and verify unified zero runtime state
  -> appsdk init <workspace> --project-root <relative-path>
```

This migration preflight is part of initialization. A new empty project with
no legacy roots skips only the legacy inspection branch; `init` must not hide
unresolved old state or conflicts.

```bash
appsdk init <workspace> --project-root <relative-path>
```

`init` is idempotent. The workspace may contain legacy code; the configured relative path becomes the new AppSDK project root. It creates the four governance zones and `.appsdk-control/` under that root, fills only missing governance files, and appends one managed `.gitignore` block for local control state, compiled Active libraries, and generated outputs. It preserves existing project files and existing ignore rules. Absolute paths and `..` traversal are rejected. Use `appsdk new` only for an empty destination.

## New feature and new project admission

New feature work and new project work require a complete design contract before
implementation:

```text
requirements and acceptance criteria
  -> confirmed goal/scope/non-goals
  -> top-down module/resource map
  -> high-level design
  -> detailed design and call/data-flow bindings
  -> requirements/design/module/verification consistency check
  -> clean worktree implementation
```

Every requirement must map to one owner module, every module to an allowed
boundary and design, and every acceptance criterion to a verification gate.
Missing or contradictory analysis/design is a worker-owned governance
preflight defect; implementation is not admitted until the chain is closed.

## New project flow

1. Install or reference a pinned external AppSDK.
2. Create the project from a template into an empty, non-symlinked destination.
3. Confirm `.appsdk/project.json`, `.appsdk/goal.json`, module ownership, zone roots, and `sdk.lock`.
4. Run `appsdk verify` before source changes.
5. Clarify the user goal: restate objective, acceptance criteria, non-goals, assumptions, ambiguities, and questions.
6. Do not claim, edit Playground, create a formal red test, compile, or promote while goal status is `received`, `parsed`, or `clarification_pending`.
7. After user confirmation, bind scope, owner, allowed/forbidden paths, and required gates.
8. Treat Playground as a logical lifecycle, not the physical checkout. Create a clean isolated Git worktree from the recorded base commit; both initial and candidate handoff states must be clean. Keep local worktree paths in `.appsdk-control/`, never in committed portable records.
9. Reproduce the issue at the base commit with the same recorded input hashes. Record the first divergence and baseline evidence before implementing the formal fix.
10. Commit the fix candidate and bind its commit, tree hash, diff hash, design ID, owner, scope, changed paths, and positive/negative verification evidence in FixCandidateRecord.
11. Before architecture review or delivery commit, run two distinct gates on the exact candidate: development whitebox verifies internal logic; deployment blackbox builds, installs, restarts, then verifies behavior only through the deployed public entrypoint. Unit tests, source-level CLI invocation, mocks, and relabeled whitebox evidence are not deployment blackbox evidence.
12. Record PreReviewValidationRecord. Bind candidate commit/tree, deployed artifact hash, independent full whitebox producer identity, deployment environment/entrypoint/producer, and disjoint whitebox/blackbox evidence IDs. Install and restart receipt IDs must resolve to unexpired PASS EvidenceRecords for the same candidate, artifact, environment, entrypoint, and deployment producer. The shared machine gate verifies the candidate Git tree, controlled source, rebuilt artifact identity, whitebox producer/artifact, and candidate ≤ whitebox ≤ install ≤ restart ≤ blackbox ≤ validation time; review admission, normal verify, and architecture promotion all call this same gate, so post-admission drift fails closed. Missing deployment capability is a blocker, never a skipped gate. Then require `appsdk verify --review-admission <project> --module <id>` to PASS; an agent must not start review or create the delivery commit before this admission. AppSDK supplies the admission command and contract; the host CI/pre-commit adapter must invoke it when physical Git commit blocking is required.
13. Run architecture review against that exact pre-review-validated candidate. ReviewRecord must reference the PreReviewValidationRecord, contain explicit PASS, AI confidence/rationale, and hashes of the resource, function, mainline-call, and verification maps.
14. After architecture PASS, rerun the original reproduction inputs plus positive, negative, and blackbox checks without changing candidate source. Record EffectivenessRecord. Source/tree/scope changes invalidate pre-review validation, review, and effectiveness.
15. Merge only after effectiveness PASS. For a single worker, MergeRecord proves the candidate commit is an ancestor of the recorded merge commit and that the merge commit remains on the declared mainline ref. For parallel development, follow the atomic scenario pair below.
16. Promote only when the complete worktree → reproduction → candidate → pre-review validation → architecture review → effectiveness → merge record graph is valid and evidence targets the exact module and candidate artifact.
17. Before freeze, run the module's declared regression suite and create a RegressionReport bound to the exact merged source commit, artifact hash, public API hash, scope hash, and input hash. Regression and bug-reproduction evidence must combine whitebox and blackbox coverage; unit and focused tests may be whitebox only.
18. Compile the merged source library, publish the immutable Active artifact, archive source/contracts to Protected, create FreezeRecord with the RegressionReport ID/hash, and verify.
19. After freeze, ordinary full-regression execution for that unchanged module may be disabled to reduce CI load. Keep the suite and report. Any source, contract, public API, artifact, or dependency input change invalidates the report and requires regression re-enablement before a new freeze.
20. Close every experiment with a PlaygroundCleanupRecord; archive evidence to Protected history, then remove the experiment directory under the declared retention policy.

## Parallel development scenario pair

`multi_worker_collaboration` and `multi_worktree_merge_queue` are one atomic capability. Enable both or neither in `.appsdk/project.json`; one-sided activation is invalid.

- Each worker owns one semantic claim, one branch, and one clean isolated worktree. A worker never edits main and never shares a worktree.
- Decompose a parent task into small independently verifiable milestones. One milestone owns exactly one claim, branch, and clean worktree. Commit and queue it immediately after its gates pass; do not stack another milestone in the same worktree or leave a completed milestone only on a worker branch.
- Start a dependent milestone only from a new clean worktree after the predecessor milestone has a live remote-main receipt. Bind the predecessor collaboration and receipt IDs; sequence 1 uses `none` for both.
- After candidate verification, architecture PASS, and unchanged-source effectiveness PASS, the worker emits CollaborationRecord and enters the serial merge queue.
- One merge owner admits one queue entry at a time. Conflict resolution is not allowed in the queue; return the issue to its owner worktree and invalidate stale candidate/review/effectiveness evidence.
- Build an integration commit from the current main base and candidate. IntegrationRecord binds that exact commit/tree and the affected verification gates.
- Merge only the tested integration commit. MainlineReceiptRecord must bind the host VCS producer, remote name/ref, observed commit, and observation time. Verification checks local reachability and queries the remote with `git ls-remote`; local tracking refs and self-declared booleans are not remote truth.
- PromotionRecord references queue, integration, and mainline receipt records. Cleanup and claim release are forbidden until remote receipt passes.

For a frozen module change, run `appsdk begin-version <project> --module <id> --from <current> --to <new>` before formal source edits. The command must bind and preserve the current Active artifact, Protected archive, and record graph; direct edits to the old Active or Protected version are forbidden.

When migrating a 0.1.5 project to AppSDK 0.1.6, run the 0.1.6 binary's `pin-lock` once. SDK canonical maps and project governance maps are separate resources: only maps that exactly match the 0.1.5 SDK canonical source are migrated to the 0.1.6 canonical target. Custom project maps are snapshotted, bound, and preserved in place; `pin-lock` must never overwrite them. Do not copy new maps or edit ReviewRecord hashes. Frozen ReviewRecord hashes resolve through the immutable migration snapshot. If an earlier 0.1.6 pin stopped after updating project/lock, rerun the same command; exact source maps or the existing migration record are required, and mixed/drifted state fails closed.

`rehydrate-frozen` is transactionally resumable. Never delete its partial generated/Protected/Active outputs by hand. Rerun the command: it resumes only an exact marker-owned module/version/artifact projection, or idempotently verifies a fully complete exact projection. Unowned partial Active state and any hash mismatch remain hard failures.

In a clean checkout, ignored generated and Active projections may be absent. Never copy them from another worktree or hand-build their records. Ensure the current Protected archive is not ignored, then run `appsdk rehydrate-frozen <project> --module <id>`. AppSDK derives the current version from FreezeRecord, rebuilds with the declared build command, requires the rebuilt artifact hash to match the immutable freeze/promotion graph, reconstructs Protected and Active projections, and runs full verification. Committed source drift, an ignored Protected archive, hash mismatch, or missing previous version history fails closed. Only after rehydrate passes may `begin-version` open the next version.

When the project is pinned to AppSDK 0.1.5, execute the target 0.1.6 binary itself and pass that exact file to `pin-lock` before rehydrate. This is the only supported 0.1.5 → 0.1.6 migration; it advances Bundle resources, lock, and project version together. Never hand-edit version fields or run an older CLI against a newer `--binary`. Unsupported source versions and byte-mismatched binaries fail closed.

A clean worktree may no longer contain the local issue branch named by an immutable MergeRecord. AppSDK resolves that recorded name only when the exact ref exists or exactly one remote-tracking ref has the same branch name. Missing or ambiguous matches fail closed; never create a local branch or choose a remote to make verification pass.

## Debug flow

Clarify goal first. Debug evidence must retain the reasoning chain: active
hypothesis, confirmation/falsification signals, first divergence, error
evidence, experiment conditions, intervention, observed result, and root-cause
decision. Positive intervention and negative/reversal experiment are required
when feasible; the final error is not automatically the root cause.

Before merge, perform an architecture check against the resource/function/
mainline maps, module registry, allowed/forbidden paths, payload/control
separation, unique ownership, and duplicate-implementation rules. Recheck the
integrated latest-main tree; stale or conflicting evidence invalidates delivery.
The physical checkout is a clean isolated Git worktree; the logical mutable
phase is Playground. Formal order is immutable: requirements/design admission
→ reproduce → evidence-backed fix candidate → development whitebox →
build/install/restart → deployed-entrypoint blackbox → architecture check →
pre-review validation PASS → selected review tool PASS → unchanged-source
effectiveness replay → verified mainline merge → promotion/compile/freeze.
Review or delivery-commit admission before the architecture and verification
gates is forbidden.

## Required checks

- `appsdk verify <project>`
- `appsdk verify --review-admission <project> --module <id>` before any architecture review or delivery commit
- project tests and required gates
- candidate artifact hash and public API hash
- record graph references, freshness, scope, module, and version relations
- clean isolated worktree identity, candidate Git tree identity, architecture map hashes, post-review unchanged-source effectiveness, merge ancestry, tested integration identity, and local/remote mainline receipt when parallel scenarios are enabled
- RegressionReport whitebox + blackbox coverage, non-zero passing tests, exact input binding, and FreezeRecord report hash
- Protected and Active immutability
- final review with explicit PASS from Jason's selected review tool, or the default route when no tool was specified

Do not claim lifecycle completion from unit tests alone. A missing external adapter, review verdict, installation, restart, or online evidence is an explicit remaining gap.

## Admission failures and upgrade recovery

`verify --review-admission` is intentionally fail-closed and read-only. On a
missing lifecycle record it emits `REVIEW_ADMISSION_BLOCKED` with `missing`,
`present`, `producer`, `next`, `retry_allowed: false`, and forbidden actions.
Treat that object as the current state contract. Do not retry, poll, hand-write,
copy, relabel, or hash-edit records. Re-run only after the named project
adapter has produced the real external evidence.

The complete adapter-owned flow is:

```text
candidate -> lifecycle adapter
whitebox -> whitebox adapter
install/restart -> deployment adapter
deployed public-entrypoint test -> blackbox adapter
bind validation -> lifecycle adapter
AppSDK review-admission verify
```

Adapters are idempotent for an unchanged candidate and preserve existing PASS
records. Any candidate/tree/artifact/environment/entrypoint/input change
invalidates the set and requires a new set. For an old-version project, first
run idempotent prepare/init, inspect and snapshot every legacy governance root,
obtain explicit object-level transfer authorization, then run the exact target
SDK migration/pin-lock. Do not delete historical evidence or merge roots. The
target binary must provide or enable the project adapter contract before the
migration can enter review admission; a missing adapter is a real blocker with
one next action, not a reason to weaken the gate.

`init` is deliberately idempotent but does not admit compilation: it leaves the
project in `draft`. The worker must confirm the goal, then execute the ordered
transitions `source_implemented -> contract_bound` for the project (and bind the
module contract) before `appsdk compile`. An early compile returns structured
`COMPILE_BLOCKED` state with the ordered next commands, `retry_allowed: false`,
and forbidden manual artifact/stage edits. Do not rerun it until the stage has
changed.

## Review tool selection

Review is a required lifecycle gate, but the tool is not hardcoded. Honor
Jason's explicitly selected review tool when it supports a read-only,
observable, structured verdict. If no tool is specified, use the configured
default review route. Record the selected tool, exact commit, scope, verdict,
and evidence. Do not silently substitute another reviewer when the requested
tool is unavailable.

Read `docs/design/appsdk-project-integration.md` for the repository layout and `docs/design/playground-active-promotion.md` for promotion semantics.

Read `references/goal-prompt.md` when Jason gives a new goal and asks you to turn it into an executable `/goal` prompt.
