# D2-A admission/receipt/query map acceptance — 2026-10-09

Task: `collab-context-identity-peer-crud-remediation-20261008`  
Candidate: `/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008`, HEAD `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`  
Worker: fresh GCM Codex thread `01a11f1d-23ff-78e1-be43-87d21abe4233`, session `35112`; final event `turn.completed`. The worker ran with `--sandbox read-only`; process handles have exited. No public product/daemon commands or identity writes were run.  
Report: [`d2-admission-receipt-map-20261009.md`](d2-admission-receipt-map-20261009.md)  
SHA-256: `d708e33e3e4573987fc09ae71280c98c7649f33499b7ddd6c764ce02f2a45918`

## Decision

Accept `READY_FOR_CONTRACT` for the D2-B design step only. The report answers the required host-local admission, outer operation receipt, actor binding, producer/reducer/replay, owner map, exact gaps, A3/A4/A5/A11 effects, and remaining unknowns. Parent spot-check of the candidate confirms the cited order: `IdentityContext` is dispatched before regular route admission; `CommandStarted` is ignored by the receipt reducer; the `CommandCompleted` event projects a completed receipt; replay rejects an incomplete started command; regular `CommandEnvelope` requires an existing actor binding. These are static source facts, not behavior verification.

## Admitted next step

D2-B may now freeze the approved identity recovery contract and context/pane-route graphs, using the accepted map as its source boundary. It must preserve `identity_context` as the single identity owner, avoid a fabricated `actor_binding_id`, and specify an outer operation ID before side effects, phase persistence/replay, and a query that does not require the expired credential. The nested Register receipt cannot stand in for that outer operation.

This acceptance does **not** admit B23 implementation, F2 product edits, D3-RUC, Create, runtime/identity actions, installation, or a design-review PASS. D2-B's contract/graph and registry must still pass separate independent design review before any implementation. A6 remains BLOCKED.

## Limits retained

- The worker did not run tests, live consumers, daemon, sockets, or host operations.
- The route, local credential file, grant, lease, and context snapshot are distinct commit boundaries; no unified outer query currently binds them to one identity operation.
- The choice of receipt extension/interface, exact query authorization, and host/project journal cross-boundary order remain design questions, not accepted solutions.
