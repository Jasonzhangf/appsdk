# r3-projection result

Status: SCOPED WORK COMPLETE for the missing projection within P02. Not an overall DONE claim.
Parent owns combined validation, architecture review, install/restart, main integration and cleanup.

## NEW deltas (production only)

Changed (received inputs; inherited P02 + r2-source changes preserved):

- `collab/src/server/mod_parts/part_08.rs` (shared projection)
  - NEW `pub(crate) fn scope_view(route_scope: Option<&RouteScope>) -> serde_json::Value`.
  - `master_authority_view` nested `scope` now uses `scope_view`.
  - `handle_master_status` emits top-level `"scope"` and keeps `master:null`/`recorded_unusable:null` on Empty.
- `collab/src/server/mod_parts/part_07.rs` (receipt assembly only)
  - promote and delegate receipts add top-level `"scope": scope_view(Some(&route_scope))`.
  - clear receipt reuses `scope_view` (inline duplicate removed; still emits scope).
- `collab/src/server/mod_parts/part_09.rs` (context projection only)
  - `handle_context` response adds top-level `"scope": scope_view(route_scope.as_ref())`.
- `collab/src/server/board_handlers.rs` (board read assembly only)
  - `handle_board_show` response adds top-level `"scope": scope_view(route.as_ref())`.
- `collab/src/dashboard/app.js` (scope display only)
  - reads top-level `snapshot.scope`, displays it for the empty slot and the holder; keeps per-worker
    online/cold/offline observations separate. Panel stays GET-only.

`git diff` for these five files also shows the inherited P02 + sealed r2-source changes; those are NOT my deltas.
No tests, docs, or other source touched. No new files created in the repo.

## Minimal owner / serializer design

- One scope builder: `part_08.rs::scope_view`. It is the single place that builds
  `{ "project_scope", "app_scope_id" }`, returning JSON `null` for an unresolved route.
- Exact route scope is exposed in BOTH Empty and Assigned states via the top-level `scope` on
  status/context/board and on promote/delegate/clear receipts. `master:null` is preserved for Empty; the
  assigned nested `scope` remains a projection of the same owner.
- A genuine unregistered route projects `scope:null`; no default app scope is guessed. Actual route/reducer
  errors return an explicit `Resp::err` before any projection, so they never masquerade as empty.
- No new state, schema, epoch, ledger, fallback, force, or panel write path. Role/policy fields and
  sender/recipient/binding/generation guards unchanged.

## Commands / exits / logs (raw under records/logs)

- `cargo build --manifest-path collab/Cargo.toml --bins` -> EXIT=0 (build.log).
- `cargo test --manifest-path collab/Cargo.toml --test tmux_recv_e2e master_authority_` -> EXIT=0,
  3 passed / 0 failed / 6 filtered (tmux_recv_e2e.log).
- `cargo test --manifest-path collab/Cargo.toml --test appserver_two_tui_integration master_authority_` ->
  EXIT=0, 1 passed / 0 failed / 14 filtered (appserver_two_tui_integration.log).
- `node --check collab/src/dashboard/app.js` -> EXIT=0.
- `git diff --check` -> EXIT=0.

## Remaining boundaries (not verified here)

- Public/live: top-level `scope`-field blackbox checks on the final combined source belong to parent/r3-public.
  The inherited smoke filters above pass but do not assert the new top-level field.
- Live runtime: no install/restart/daemon action taken (parent-owned).
- Main: no commit/merge/push; combined validation and independent architecture review are parent-owned.
