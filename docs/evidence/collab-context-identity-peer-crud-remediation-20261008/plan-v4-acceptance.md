# Planner v4 acceptance

Task: `collab-context-identity-peer-crud-remediation-20261008`  
Planner run: fresh `codex exec --profile oauth --model gpt-6.1-sol --sandbox read-only --ephemeral`; session `53821`, completed exit `0`.  
Plan: [plan-v4.md](plan-v4.md).  
Observation: [o4-managed-fixture-observation.md](o4-managed-fixture-observation.md).

## Decision

Accepted as the current executable plan for design preparation and read-only A6 unblocking. `READY` applies only to this planning handoff and the two next nodes below. It does not grant product implementation admission.

Keep four user capabilities, F1–F4 and A1–A12 unchanged. Accept the dependency correction: F1's CLI/MCP dispatch and response-preservation code can be designed and implemented before a public managed fixture exists; public managed Send/Ready success, partial and repair black-box evidence remains dependent on A6's real Create lifecycle. Private `Record`/journal seed and ordinary peer registration do not count.

## Next nodes

1. **D1-R4**: update the F1 contract and registered design graph for CLI no-op dispatch, write-capable identity reconciliation, route/admission, complete `Resp` propagation, phase results, and exact public black-box gates. Validate the revised graph and registry before a new independent design review. Keep `collab-f1-design-review-20261008-v2` as immutable FAIL; use a fresh review task. Do not edit product source in this node.
2. **O5-R4**: read-only inspection of Native create correlation, existing typed command receipts, public partial-result readback, and the remaining create unknown window. Do not run a provider, start/replay a thread, create a peer, or mutate identity/runtime state. If the association gap remains, return the smallest evidence-based unblock observation; do not issue Create implementation READY.

Only after D1-R4's new design review PASS may I1 implement the F1 public dispatch/response portion. C1 remains an unintegrated candidate until A6 permits the required public managed behavior and the complete A9/A11 gates pass. The plan's suggested `collab worker create --kind peer|managed` shape is a design candidate only; no command, parameter, operation key, receipt schema, or lifecycle owner is frozen before D3 and O5 evidence.

## Boundaries and unresolved items

- A6 remains `BLOCKED` on the create-response correlation/recovery window. Native ordinary-thread start/update/archive evidence is not proof of safe Collab managed creation.
- Full F1/A9 managed success remains `INCOMPLETE` until public Create/Bind/Route exists and a real CLI/MCP consumer reads durable message, task, repair/wake, and consumption outcomes.
- F1 implementation must preserve the `me()` identity-commit versus requested-action outcome boundary; do not claim action rejection means no identity changes.
- The prior design review FAIL remains in the record and cannot be retried under a new name to bypass its findings. The new review must bind revised graph/contract hashes and the updated verification evidence.
- Existing main-tree dirty work and other worktrees remain outside ownership. Product implementation stays in the task's external candidate worktree.

## Current status

No product source changed in this round. Next action is to issue independent D1-R4 and O5-R4 contracts with disjoint write scopes, then receive both results. Installation, daemon maintenance, live managed fixture, implementation review, Git integration and cleanup remain not started.
