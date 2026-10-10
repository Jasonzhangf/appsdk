# D2/D3 v2 planning acceptance

Date: 2026-10-08 America/Los_Angeles
Task: `collab-context-identity-peer-crud-remediation-20261008`
Planner output: `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/planner-d2d3-v2/plan.md`
Input: current observation addendum and the v4/F2-F3/O2/O5 evidence listed by the plan.
Planner invocation: fresh `/opt/homebrew/bin/codex exec --profile oauth --model gpt-6.1-sol --sandbox read-only --ephemeral`; startup and completion are recorded in sibling `events.jsonl`, with MCP startup warnings in `stderr.log`.

## Decision

Accept **READY for planning and design only**. The recommendation keeps the four user goals, F1-F4, A1-A12, installed/live delivery, and A6 Create requirement intact. It does not authorize D2/F2 or D3/F3 implementation, Create, installation, daemon changes, or user identity operations.

Keep D1-R4 and its exact review PASS as the F1 design result only. Keep F1 as a partial candidate: its current unit and static consumer results do not close daemon-backed public behavior, installed/live behavior, or implementation review. Keep A6 **BLOCKED** until stable request-to-thread association or exact owned cancellation and durable public query cover response loss and restart.

## Accepted direction

- D2 retains `ProjectRuntimeManager::identity_context` as the single identity decision owner. A host-local approved recovery path must enter before the stale `me()`/binding path, then reuse the existing registration, route, credential, grant, and lease owners. It must not fabricate a binding, mint an identity because recovery failed, merge identity restoration with master grant replacement, or disclose a token.
- D2 must freeze one typed operation intent, exact project/app scope, target identity, explicit approval, phase/partial outcomes, idempotency, restart readback, and query authorization before public CLI/MCP fields or implementation are admitted.
- D3 uses one daemon lifecycle owner and the actual selected host adapter. Read is an observation. Initial Update scope is `cwd`, with real subsequent execution evidence. Close needs a responsibility fence plus both exact-host stop and control-plane retirement receipts. `WorkerClosed`, archive ACK, or a closed record alone is insufficient.
- Design Read/Update/Close for a real ordinary peer where ownership can be proven. Preserve PR #17's confirmed Missing close boundary. Do not convert Unknown to Missing or claim every ordinary peer has a controllable host instance.
- Keep Create outside D3-RUC implementation until A6 is independently resolved and receives its own design admission.
- Keep context/help/MCP/Skill operation cards downstream of the accepted D2/D3 contracts. Do not reduce CRUD requirements to match current gaps.

## Controller refinements

The planner proposed O2-B, O3-B, and O5-U as minimal source/evidence tasks. Apply them as follows:

1. Reuse the existing O2 identity observation, accepted F2/F3 plan, O5 correlation observation, and their cited raw receipts. Do not repeat those reads or runtime probes merely to create new notes.
2. D2 may proceed from existing O2 evidence. Re-open only the exact source slice if the candidate changed or the named receipt/reducer/query owner remains unclear. Record a specific missing symbol or transition before expanding observation.
3. D3 may proceed from the prior plan's close observations and O5/host evidence. Run a narrow read-only O3-B only for uncovered responsibility writers, stop/query owner, or binding/route retirement events. No daemon-backed probe is allowed in the current socket-restricted environment.
4. Treat the F1 checkpoint's AF_UNIX/loopback `EPERM` as an environment limit, not a product verdict. Do not route around it or repeat the same probe.
5. Reconcile the plan's diagram registry task with the existing dirty `docs/dagpipe/manifest.json`, `rust/src/dagpipe.rs`, and `rust/src/dagpipe_tests.rs`: current run records show these exact D1-R4 edits were made under this task and no D1 process remains active. The parent now takes sole ownership of these registry paths for G23 integration. D2/D3 workers may write only their distinct contract/graph files and must not edit registry files. Parent integrates graph registrations after both graph proposals are received.

## Registry ownership correction

The earlier run-note warning described these files as potentially belonging to another task. Re-reading `design-f1-v4/worker-task.md` and current diff proves the manifest, embedded graph list, operator inventory, and tests were part of the D1-R4 allowlist. Process inspection shows no current D1 writer. Therefore G23 is owned by the parent within this task; the previous “wait for external owner transfer” condition is withdrawn. Do not modify unrelated dirty files or the main worktree.

## Next accepted nodes

| Node | State | Gate |
|---|---|---|
| D2 identity contract and graph proposal | IN_PROGRESS | Fresh worker `design-d2-identity` writes only D2 contract/anchor/context/pane-route design files |
| D3 Read/Update/Close contract and graph proposal | IN_PROGRESS | Fresh worker `design-d3-peer-lifecycle` writes only the new lifecycle contract/graph |
| A6 Create resolution | BLOCKED | No retry/replay; requires stable association or owned cancellation plus durable query proof |
| Registry integration and DR23 | WAITING | Parent owns D1-R4 registry files; integrate both graphs after return, validate, then run separate D2 and D3-RUC design reviews bound to hashes |
| B23 and upper implementation | NOT AUTHORIZED | Requires design PASS; no shared receipt/admission code or CLI/MCP wiring before then |
| F4 operations cards | WAITING | Freeze D2/D3 public contracts first; Create stays visibly blocked until A6 closes |

The planner's ownership, paths, commands, behavior gates, stop conditions, and delivery sequence are retained in the full plan. These refinements remove repeat observations and block shared dirty registry writes; they do not change the planner's core design contracts.

## Evidence and limits

- Plan status: READY for planning/design only.
- Independent design review: not run for D2 or D3.
- Product implementation: not authorized by this acceptance.
- Runtime/daemon/installed/live: not run by the planner.
- Goal: active / INCOMPLETE.
