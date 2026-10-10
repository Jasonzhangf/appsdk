# O5-R4: create correlation and durable readback

Status: **A6 BLOCKED**. This is a read-only source/schema/receipt observation; no provider, thread, peer, identity, CLI consumer, or daemon mutation ran.

## Findings

- Native `thread/start` has no client thread ID or thread-level idempotency key. `thread/started` is an unsolicited notification without the originating JSON-RPC request ID. A project idempotency key identifies the project, not the new thread.
- The live raw receipts show that empty `thread/list` or `thread/loaded/list` results do not prove a create did not happen. A later observed thread could not be resolved through the attempted project listing. The request-to-thread association window remains open.
- Collab's typed command receipt can replay a completed command ID, but `CommandStarted` is framing only. There is no persisted started/pending/unknown lifecycle stage and no public restart-safe receipt query. It cannot currently reconcile an unknown `thread/start` result.
- Durable message status exposes accepted wake evidence and consumption, but notification failure, escalation, and `repair_required` do not have a public durable readback path. They remain response-only at this baseline.

## Impact

- A6 peer creation stays blocked until the exact sent-but-unassociated window has either a stable durable request-to-thread lookup or a proven, owned cancellation terminal with cleanup receipt.
- Managed Send/Ready success, partial, and repair black-box evidence for A9/A11 stays unverified until A6 provides the public Create/Bind/Route lifecycle and public durable readback.
- Do not retry `thread/start` after an unknown response, infer correlation from an empty list, invent an idempotency field, or seed private state to claim public success.

## Parent reassessment — 2026-10-10

The source and raw receipt facts above remain valid. The previous A6 impact statement was too broad: it treated the lost-response lookup gap as a prerequisite for every Create outcome. On a received success response, JSON-RPC response `id` matches the `thread/start` request and the actual `thread.id` is available; see `native-capability/inject-resume-receipt.json:65-81` and `collab/src/adapters/codex_app_server_production_part2.rs:436-459`. This is enough to proceed with a bounded synchronous success path.

Accepted Create boundary: persist one durable dispatch claim before sending `thread/start`; never resend after that claim, including on replay or restart. Persist the returned exact thread ID before registration/binding/route work. If the response is lost or the daemon stops after dispatch without an ID, report a durable queryable `unknown` and refuse another dispatch for that unresolved operation/target. Do not infer or claim the unknown thread, and do not count this state as successful Create. The existing host has no proven way to locate or clean up that unknown thread; retain this limitation explicitly.

This is a parent accepted plan amendment, not implementation acceptance. It narrows A6 from `BLOCKED` to `READY_FOR_BOUNDED_IMPLEMENTATION`; A6 behavior stays `INCOMPLETE` until successful Create, exact query/replay, and unknown/no-resend cases pass through public CLI/MCP against an isolated real daemon. No Native API fields or retry semantics are invented. The same canonical `main` + app-scope association rule is unchanged.

## Evidence source

Full read-only observation and raw receipt paths are retained in `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/observe-create-contract-v4/`. The worker could not write its read-only run directory and returned the result inline; this parent-owned record preserves that result. Native capability receipts are in the same task's `native-owner/` and `native-capability/` run directories.
