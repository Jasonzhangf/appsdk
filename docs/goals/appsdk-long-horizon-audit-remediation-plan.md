# AppSDK Long-Horizon Audit Remediation Plan

## Source audit

- Source: `/Users/fanzhang/Downloads/appsdk_long_horizon_audit_5a25998_2026-09-07.md`
- Audit date: 2026-09-07
- Audited baseline: `5a259985b3e2e0828f84f721cf3f7a856e340cbe`
- Scope: long-horizon execution, role governance, goal subscription, bug
  adapter correctness, Guidance persistence/recovery, notification contracts,
  and related tests.
- Audit limitation: the report did not modify the repository or run local
  tmux/Collab replay, fault injection, or long-stability experiments.

## Goal

把 AppSDK 长程执行收敛为角色正确、失败显式、目标订阅可恢复、通知合同唯一、
Guidance 可重放、证据可验收的执行系统；保留 Collab 作为运行态所有权真源，
不增加第二个 daemon、任务队列或隐式调度注册表。

## Acceptance criteria

### Control correctness

- `longhorizon show` and all role briefs are projected from verified Collab
  identity, parent, task, and authorization.
- `master`, independent `worker`, managed `subagent`, and unknown identity
  receive distinct executable views.
- Unknown identity never receives master capabilities or becomes master through
  a flag, prompt text, or missing state.
- `task block` propagates the underlying result. Missing task, permission
  failure, process failure, invalid output, and timeout remain explicit errors.
- Blocking a task does not silently close unrelated direct-message or human
  notification channels.
- Bug read/write failures never become an empty backlog, an implicit upstream
  write, or a successful close.

### Goal lifecycle

- Goal subscribe, status, renew, restart/recovery, and cancel have stable
  `goal_id`, revision, owner, desired state, observed Collab state, and
  idempotent reconciliation.
- Duplicate registration is explicitly rejected or replaces the prior
  subscription with an auditable result.
- Lost responses, persistence failures, expiration, daemon restart, and cancel
  failures remain observable and recoverable.
- Local intent is never reported as observed runtime success.

### Role, notification, and waiting contracts

- One structured policy source generates role briefs, notification text, and
  relevant help.
- Reading a notification consumes it; no normal ACK loop or ACK-only completion
  path remains.
- Persistent versus ephemeral subagent behavior comes from effective config,
  not duplicated constants or conflicting documents.
- Waiting is determined by reason, responsible owner, unblock condition, and
  recovery trigger. Genuine external/resource/dependency waits remain valid;
  empty or lazy `blocked` reports do not.
- Master owns blockers and must solve, reassign, or auditable-force-close;
  workers report root cause and proposed resolution without impersonating master.

### Guidance and recovery

- Plan validity is separated from attempt/evidence input validity.
- Expected source changes do not force a full plan rebuild.
- Relevant content changes invalidate the correct evidence; unrelated changes
  do not unnecessarily replay completed work.
- Concurrent updates are serialized or version-conflicted at the task boundary.
- Journal is the durable truth; plan projection is rebuildable.
- Partial writes, bad JSONL tails, crash-at-write boundaries, and replay
  failures are explicit and recoverable, never silently discarded.

### Delivery evidence

- Required format, unit/integration, fault-injection, and public-entrypoint
  tests pass.
- Latest `origin/main` is merged before final verification.
- Review is performed on the exact validated diff.
- The final binary, skills, daemon state, and applicable live replay are
  verified separately.
- Bugs contain the final commit, test results, artifact digest, and runtime
  evidence before closure.

## Scope

### In scope

- AppSDK role-aware long-horizon briefing and execution-view projection.
- `task block`, task/notification result propagation, and error contracts.
- Goal subscription persistence, reconciliation, renewal, cancellation, and
  recovery.
- Canonical role/notification policy and conflicting skill/reference/template
  guidance.
- Bug adapter read/write/close semantics and explicit repository targeting.
- Guidance plan/evidence binding, journal replay, concurrency, and crash
  recovery.
- Deterministic negative tests, fault injection, public CLI tests, and live
  Collab/daemon verification where the changed delivery object requires it.
- Updating the audit-linked bug list and recording final commit identities.

### Out of scope

- Rewriting all of AppSDK or splitting into multiple crates before control
  correctness is fixed.
- Adding a second scheduler, daemon, task queue, worker heartbeat source, or
  registration database.
- Weakening review, artifact, environment, merge, freeze, or evidence gates.
- Treating Memory, Guidance, or a prompt as a replacement for Collab runtime
  truth or canonical evidence.
- Autonomous speculative refactoring after the audit findings are closed.

## Design principles

1. Collab remains the single runtime owner for identity, role authority,
   parent/child ownership, task state, pane identity, notification lease, and
   runtime subscription.
2. AppSDK provides role-aware execution views, application use cases, evidence
   checks, and adapters; it does not invent a parallel runtime truth.
3. Control data stays typed and separate from business payload, screen text,
   logs, and prompt prose.
4. Every external operation returns typed success, failure, or unknown result;
   no implicit retry, fallback repository, empty-on-error conversion, or
   success-after-ignored-error path.
5. Desired local intent and observed remote/runtime state are stored and
   reported separately.
6. Durable journal/events are authoritative; projections and summaries are
   rebuildable caches.
7. A notification is an interrupt. Consume it, execute its action, and resume
   the owned task or legitimate scheduling responsibility.
8. Master is accountable for project outcome; workers and subagents retain
   their declared ownership and report concrete blockers.

## Technical plan and file ownership

### PR-A: Control correctness

Likely owners:

- `rust/src/main.rs`
- role/briefing helpers and their focused tests
- bug adapter and task command tests
- relevant role/notification skill and contract references

Work:

- Replace unconditional `MASTER_CHARTER` injection with a verified,
  role-specific execution view.
- Make `task block` preserve and return the underlying Collab response.
- Normalize bug labels at the boundary and represent unavailable/invalid/stale
  reads distinctly from an empty result.
- Require explicit repository/issue references for writes and closure.
- Require canonical evidence references for verified closure.

### PR-B: Goal subscription lifecycle

Likely owners:

- goal command/application code in `rust/src/main.rs` or its current owner
- goal state/record tests
- Collab goal adapter boundary

Work:

- Add stable goal identity, revision, owner, desired state, observed state, and
  subscription reference.
- Make subscribe/cancel/reconcile idempotent and auditable.
- Reconcile after missing responses, persistence failures, expiration, and
  daemon restart.
- Preserve tombstones/receipts for cancellation and failed operations.
- Do not add a second timer or scheduler.

### PR-C: Role and notification contract

Likely owners:

- canonical policy/role projection code
- `skills/collab/SKILL.md`
- `skills/collab/references/notifications.md`
- `skills/appsdk-project-governance/SKILL.md`
- subagent configuration references
- README and verification/resource/function maps

Work:

- Generate role briefs and notification guidance from one structured policy
  source.
- Remove conflicting ACK, persistent/ephemeral, waiting, and cleanup claims.
- Keep the current consume-on-read contract: `collab recv` is the normal
  consume operation; legacy ACK is recovery-only.
- Emit only the current agent's role, parent, task, allowed scope, next action,
  and legitimate wait/close conditions.
- Add role transition and child-idle regression coverage.

### PR-D: Guidance stability and crash recovery

Likely owners:

- `rust/src/guidance.rs`
- Guidance ledger/projector/storage code
- Guidance unit, integration, and fault-injection tests

Work:

- Separate stable plan contract from per-attempt source/dependency/config/
  artifact fingerprints.
- Detect content changes, including dirty and untracked file content, rather
  than relying only on Git status shape.
- Add task-bound version/lock conflict handling for concurrent updates.
- Make journal replay and plan projection deterministic after partial writes,
  duplicate event IDs, and crash boundaries.
- Treat malformed middle records as explicit corruption; do not silently skip
  history.

### PR-E: Physical boundary and cost ablation

Likely owners:

- current Rust module boundaries and registry/maps
- policy, application, adapter, evidence, storage, and CLI entrypoints
- benchmark and regression documentation

Work:

- Extract only boundaries needed to keep policy pure and adapters typed.
- Keep `main()` as argument/exit-code wiring.
- Remove duplicate policy text and duplicated lifecycle state where confirmed.
- Run controlled ablations for repeated issue creation, full prompt injection,
  unnecessary replans, full backlog scans, and mandatory child shutdown.
- Report reliability and cost measurements without weakening acceptance gates.

## Risks and mitigations

- **Role source unavailable:** return unknown/recovery view; never infer master.
- **Collab response lost:** query by stable operation ID before any write retry.
- **Local record write fails after remote success:** retain pending reconciliation
  state and recover by observed lookup; never report active/complete blindly.
- **Policy drift:** generate prose from one typed source and add drift checks.
- **Guidance corruption:** retain journal evidence, isolate the bad boundary,
  emit a recoverable error, and preserve unrelated valid history.
- **Concurrent workers touch the same owner files:** stop the affected shared
  write, establish explicit ownership, and preserve unrelated dirty changes.
- **Live replay differs from unit tests:** classify the first divergent boundary
  as identity, durable commit, daemon, tmux wake, agent response, consume, or
  lifecycle close; do not call a lower-layer pass end-to-end success.

## Verification matrix

| Area | Required evidence |
| --- | --- |
| Role projection | master, worker, managed subagent, unknown identity at the same entrypoint |
| Task block | existing task, missing task, permission failure, invalid output, timeout, unrelated notification preservation |
| Bug adapter | unavailable binary, command failure, invalid JSON, empty result, explicit upstream, comment/close failure, verified receipt |
| Goal lifecycle | first subscribe, duplicate subscribe, lost response, local persistence failure, expiration, restart, renewal, cancel failure, reconciliation |
| Notification | consume-on-read, no ACK loop, persistent/ephemeral policy, role-specific action, child idle, legitimate waiting |
| Guidance | expected dirty edit, relevant content mutation, unrelated mutation, untracked mutation, concurrent update, duplicate event, crash before/after journal and projection writes, malformed JSONL |
| Build and quality | `cargo fmt --check`, `cargo test --all`, release/build checks, skill validation, exact diff review |
| Runtime | installed binary and skill identity, controlled daemon restart, PID/socket uniqueness, required Collab/tmux live replay |
| Delivery | latest-main merge, pushed commit, bug comments with commit/test/artifact/runtime evidence, clean final worktree |

## Implementation steps

1. Query and reconcile existing AppSDK bugs for F01-F10; reuse related issues
   and create only missing defects.
2. Create an isolated owner worktree from the latest `origin/main`; register
   task and file scope through Collab when multi-worker execution is active.
3. Implement PR-A with red tests first; restore formatting and full CI coverage.
4. Implement PR-B and add deterministic reconciliation/fault-injection tests.
5. Implement PR-C; update skills, references, templates, maps, and role tests.
6. Implement PR-D; run crash/replay/concurrency tests and repair the first
   semantic divergence.
7. Implement only the PR-E extraction/ablation that is justified by measured
   duplication or policy drift.
8. Review each exact diff after its affected tests pass; fix blockers and rerun
   the impacted matrix.
9. Merge each validated batch into the latest main, rerun mainline verification,
   and push only the tested integration commit.
10. Rebuild/install the final binary and global skills; restart only the exact
    affected daemon service.
11. Run the applicable live Collab/tmux replay and record durable send,
    notification, agent response, consume, task close, cleanup, and daemon
    evidence separately.
12. Comment final evidence into the bug system, close only verified issues,
    leave unresolved architectural findings open, and remove only authorized
    residual artifacts.

## Definition of done

- F01-F10 are either fixed with evidence or remain explicitly open with a
  reproduced root cause and owner.
- No role can obtain master behavior from a generic brief, missing identity, or
  prompt text.
- No command reports success after an ignored or unknown underlying result.
- Goal subscriptions reconcile correctly across restart, expiration, duplicate
  calls, lost responses, and cancellation.
- Role, notification, waiting, ACK, and child lifecycle guidance has one
  executable contract.
- Guidance preserves valid progress, detects real input changes, serializes
  conflicting writes, and recovers deterministically from crashes/corruption.
- Required tests, review, latest-main merge, push, install, restart, and live
  replay evidence are present for the final changed delivery object.
- Bug records reference the final commit and evidence before closure.
- The long-horizon subscription is cancelled only after this DoD passes.
