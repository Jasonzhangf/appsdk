O2 source observation is complete. This was a read-only source/documentation pass with no runtime execution.

**Status: DONE (source-only observation; live capability UNVERIFIED)**

Deliverables written:
- `notes.md` — N0–N6 nodes with commands, results, and evidence paths
- `result.md` — full owner/commit-boundary table, approval-input + admission draft, legal-Update contract, credential rationale, event/receipt reuse table, explicit unknowns

**Key source conclusions** (all with confirmed locations):

- One-click flow is 6+ separate boundaries, not one transaction: CLI syntax (`main_context.rs:196`) → host admission (`identity_context.rs:8/20`) → read-only resolver (`identity_resolver.rs:115`) → committed-credential reconcile (`identity_context.rs:209`) → typed `Register` with its own receipt (`part_02.rs:225`, `part_06.rs:1050`) → separate host-route journal (`part_04.rs:418/507`) → local receipt file (`identity_context.rs:125`) → read `Context` projection.
- **Old token/binding recovery gap is real.** `reconcile_committed_credential` early-returns whenever a local `Identity.runtime` exists, so a stale local credential never gets corrected and `Register` then raises `TOKEN_MISMATCH`. There is no current approval entry that bypasses the failing resolver/`me()`. Endpoint ownership is daemon-admitted (`part_03.rs:1`, `verify_candidate` at `codex_app_server_production_part1.rs:394`); scope is validated in `identity_context.rs:31-33`; observed/provided conflict is wrongly pre-decided client-side (`main_context.rs:176`) and should move to the daemon.
- **Receipt coverage is partial.** The outer `IdentityContext` has no command receipt; only nested typed `Register` does. Route publication is a separate journal where definite errors roll back (`GlobalRuntimeBindingRollback`, `state_impl.rs:703`) but append/flush/replay/reducer errors return **result unknown** with no rollback.
- **`WorkerClosed` is insufficient for true retirement.** PR17's Missing path (`part_07.rs:1317`) commits only `WorkerClosed`; the reducer (`state_impl.rs:274`) removes worker/keepalive/idle but leaves runtime binding, current-thread route, master grant, subscriptions, and mailbox untouched. Full retirement must compose `retire_runtime_binding_after_route_failure` (`part_06.rs:889`) and/or `pane_reclaim_events` (`part_06.rs:1264`).
- **Grant rebind reuse exists.** `register_typed` (`part_02.rs:240-330`) reissues `GlobalMasterGranted` for same-principal generation+1 when the prior binding/generation/scope matches; `bind_runtime` removes the old grant in the same candidate (`global_state_impl_part2.rs:597`).
- **Create lifecycle cannot reuse the managed record.** `subagent::Record` forces `parent` (`subagent.rs:79`) and any `record.peer` makes a worker "managed" (`part_07.rs:387`); production `Start` is rejected (`subagent.rs:846`). Ordinary independent peers need a separate typed lifecycle record or explicit non-managed flag.

**Runtime / live: NOT verified.** No mock success was produced. All conclusions are source-only. No product files, tokens, journals, routes, or grants were touched; no promote/clear/takeover/restart; no Collab registration. Product stayed at `3dfdaf85` with only the pre-existing untracked task docs/evidence.

**Resources:** no processes started by this task, so no PID/session cleanup. The `identity-capability/` directory (notes, result, events.jsonl, stderr.log, worker-task.md) is retained for parent intake. I did not observe or alter the canonical daemon; `ps`/socket checks are sandbox-blocked in this session, so I make no claim about its current state.

**Minimal next actions:** (1) O1 real AppServer create→read→work→update→close with unknown-boundary query evidence; (2) design owner freezes approval typed input, admission order, legal-Update immutables, and close-retirement event composition plus independent design review; (3) prove idempotency of approval recovery against existing events/receipts before implementation.