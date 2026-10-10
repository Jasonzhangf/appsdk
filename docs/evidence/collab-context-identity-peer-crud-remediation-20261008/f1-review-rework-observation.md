# F1 design review rework observation

Status: source-verified observation for re-planning; no product source changed.

- Task: `collab-context-identity-peer-crud-20261008`
- Review: `collab-f1-design-review-20261008-v2`, Codex `oauth/gpt-6.1-sol`, terminal `fail / code_failure`, two P1 findings.
- Candidate base: `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`; D1 graph/contract are uncommitted design changes only.

## Finding 1 — identity context is a write-capable preflight

At `collab/src/main.rs:47-62`, `me()` resolves the project scope, calls `main_context::identity_context_response`, then parses its `identity_receipt` and `runtime_for_request`. That helper (`main_context.rs:227-250`) sends `Req::IdentityContext` to the daemon. In the daemon owner (`server/identity_context.rs:88-147`), complete facts resolve an identity, dispatch `Req::Register`, persist registration locally, and dispatch `Req::Context`; missing facts return a non-registered result. Therefore a CLI command using `me()` may commit identity/binding/route/receipt before its requested subagent action is denied or before the action fails.

F1 contract sections 4–5 currently describe `me()` as reading a daemon-owned receipt and classify admission/action failures as having no persistent side effect. That is inaccurate for an unregistered caller with complete facts. The observable contract must distinguish identity reconciliation committed from the requested action not executed, and explicitly bound which action-owned journal/mailbox/task effects remain absent. A fixture should prove both: missing facts cause no registration; complete facts can register before a later action denial, without claiming the action ran.

## Finding 2 — existing Send/Ready paths discard notification response data

`collab/src/subagent.rs:412-437::notify` calls `server::handle_send_with_task`; on non-OK it propagates only `response.error` as a string and discards `response.data`. The notification owner in `server/mod_parts/part_05.rs:1077-1134` places durable message identity, `repair_required`, escalation, and failure class in `Resp.data` for a failed/repair-required notification. The `Send` action (`subagent.rs:1061-1089`) persists an idle reconciliation before notification and then can return only the text error; it cannot expose the message ID or repair fields.

`Ready` (`subagent.rs:995-1028`) may commit the child status before notification. It applies `?` to `notify`, losing error data on rejection; if notification has no subscription, the notification owner returns an OK repair projection with `repair_required`, but `Ready` ignores the successful value and returns the ordinary subagent object. Thus CLI/MCP wiring alone cannot satisfy the current design promise to report durable/partial/repair facts.

The smallest candidate repair must preserve the existing `Resp` envelope (including `ok`, `error`, and `data`) across `notify` to the existing `Subagent` result, without adding a second ledger, automatic retry, rollback, or false consumption. The public projection must report committed message/status, wake result, and repair next step separately. Planner must decide the minimum owner and allowed paths before implementation.

## Scope consequence

The current I1 allowlist excludes `collab/src/subagent.rs`, yet this is the unique owner where the second finding occurs. The accepted plan must be amended before implementation. Reuse the current notification owner and public error/data projection; do not change peer lifecycle, identity protocol, or A1–A12 requirements. The F1 design graph must show that identity reconciliation can commit before an action outcome and that notification/result projection has a real owner. Re-review the revised design before editing product code.

## Exact evidence

- Review final: `/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/.agent-collab/review/collab-f1-design-review-20261008-v2/review.final.md`
- D1 contract: `f1-design-contract.md` (reviewed candidate; revise after replan)
- Baseline sources: `collab/src/main.rs:47-62`; `collab/src/main_context.rs:227-250`; `collab/src/server/identity_context.rs:88-147`; `collab/src/subagent.rs:412-437,995-1089`; `collab/src/server/mod_parts/part_05.rs:1077-1134`.
- No identity, route, grant, journal, mailbox, or task state was changed during observation.
