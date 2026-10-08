## Design Admission Review — Collab master authority (2026-10-07)

**Verdict: PASS (pre-code design admission).** No P0/P1 blockers. The contract and three graphs are an implementable, minimal, single-owner design consistent with the accepted plan and with the base observations. Topology validations are shape-only and I treated them as such; I corroborated the load-bearing semantic claims against source.

**Evidence read (paths)**

- Contract: `docs/design/collab-master-authority-contract-20261007.md` (§1 scope, §2 interfaces, §3 matrix, §4 terminals, §5 transactions, §6 recovery, §7 projection, §8 boundary, §9 replay/init, §10 DAG, §12 matrix).
- Graphs: `docs/dagpipe/collab-master-authority.graph.json` (3 nodes, linear), `collab-context.graph.json` (6 nodes, linear), `collab-dashboard.graph.json` (4 nodes, linear). `dagpipe graph validate` logs all `valid DAG … operator bindings syntactically present; project compile() remains the authoritative … gate` in `design-revision/validate-*.log`; `inspect` shows waves 1→2→3 and no back edge.
- Plan: `plan/plan.md`; prior review `design/review-v1.md`; parent `note.md`.

**Source seams corroborated (targeted, not whole-repo)**

- Scope + typed grant fields: `collab/src/server/global_state_models.rs:558-604` — `MasterGrant { project_scope, app_scope_id, agent_id, boundary, granted_by, approval, binding_id, endpoint_generation, granted_at_ms }`. Exactly the fields §7.1 projects; no `revision`, no `request_id`. Contract's "no fabricated revision / no schema change" holds.
- Existing atomic transfer helper: `part_06.rs:1264-1285` (`master_authority_transfer_events`) already revokes every current grant in the exact `(project_scope, app_scope_id)` then issues `GlobalMasterGranted` in one commit — §5.1 reuses this; §5.2 clear is the same revoke set without re-grant.
- Actor admission: `part_10.rs:811-900` `project_route_actor` validates token, project scope, unique current binding, `endpoint_generation != 0`, session/thread binding; `MasterPromote`/`MasterDelegate` are already in the mutation list (`part_10.rs:412-413`, `520-529`). `MasterClear` slots in identically — §2.3/§5.2 admission claim is real, not aspirational.
- Same-principal reissue: `part_02.rs:245-339` — `same_principal`, grant reissue gated on old binding/agent/scope/generation, `GlobalRuntimeBound` + `GlobalMasterGranted` in one transaction. §6 maps to this owner; the same-pane/same-DSH exceptions (`part_02.rs:~283-311`) and the recovery live fence (`part_06.rs:1090-1116`, `MASTER_RECOVERY_BLOCKED_LIVE/UNKNOWN`) are the exact §6/§8 deletion targets and they exist.
- Reducer/replay safety for "reuse existing revoke, no new clear event": `state_impl.rs:683-702` — `GlobalMasterRevoked` removes the typed grant **and** clears `master_worker_id/assigned_by/approval/assigned_ms` when the revoked agent is the legacy master; `part_12.rs:330-344` `track_legacy_master_authority` sets `legacy_master_is_current=false` and `saw_typed_master_authority=true` on any typed grant/revoke, and the legacy→typed import at `part_12.rs:516-556` only fires when `legacy_master_is_current` is true. Therefore a committed clear cannot be resurrected by a later legacy `MasterAssigned` on replay. This directly answers the one genuine design uncertainty (can existing revoke express empty?) — yes, so §5.2's "stop and replan only if replay red test disproves" gate is sufficient and the parent's rejection of a new clear event/ledger/epoch is safe.
- Legacy fallback in public reads (the bug surface §7 removes): `part_08.rs:12-15` (`.or_else(|| state.master_assigned_by/approval/ms)`), `part_08.rs:92` (`state.master_worker_id` fallback), `part_08.rs:97` `recorded_unusable`; `part_09.rs:1281-1413` `(master, recorded_unusable)`. Confirmed present.
- Status fails on unknown today: `part_08.rs:75-88` `handle_master_status` returns `Resp::err_data` when `live_master_id` errors, and `part_07.rs:403-424` returns `Err` for `IdentityPresence::Unknown`. §3/§7 (status always succeeds with authority + separate transport) is the correct minimal fix.
- Live authority gates named in §8 all exist: `board_handlers.rs:139-141` (`BOARD_LIVE_MASTER_REQUIRED`), `part_07.rs:1198,1276,1331,1422` (`verify_master_actor`), `part_08.rs:1448` (`task_integration_authorized`), `part_09.rs:66,229,361,507,798`, `part_04.rs:975`, `subagent.rs:898`, `main.rs:680` (`endpoint_live`), `part_06.rs:64` (deadline/master-idle). No dangling references.
- Public surface to wire: `proto.rs:604-611` has `MasterPromote`/`MasterDelegate` but no `MasterClear`; `part_11.rs:460-470` dispatches promote/delegate/status; `collab-mcp.rs:158` enum is `["status","promote","delegate"]` (no `clear`); `main_cli.rs:174` still says "when no live master exists". Matches §11 pending edges.

**Requirement-by-requirement**

- Exact `(project_scope, app_scope_id)` preserved: §1, §3, §7; corroborated against the real grant model. Pass.
- `clear --approval` / `Req::MasterClear { worker_id, token, approval }` / MCP `action=clear` through current-actor admission, atomic revoke only, peers/tasks/mailbox preserved: §2.1-2.3, §5.2, §4 Cleanup terminal. Pass.
- Same-principal proof via persisted token/identity/binding, no pane-only rights: §6; corroborated against `part_02` transaction. Pass.
- Controls read current typed grant; transport probes only actual communication: §3, §7, §8. Pass.
- Holder stays assigned on Unknown/Cold/Missing: §3, §7. Pass.
- Common status/context/board grant projection, GET-only panel: §7, §10. Pass.
- Terminals Empty/Assigned/rejected-unchanged/persistence-error/lost-response-query/restart/cleanup: §4. Pass.
- Concrete consumer/negative/side-effect matrix: §12 B01-B15 (success, negative, and data-preservation side effects). Pass.
- 3/6/4-node SESE preserved; validations are topology-only: §10, §11; graph files and logs. Pass.
- No invented mechanisms: §1 explicit "不引入" list (no request_id, no ledger, no new clear event, no second epoch, no force, no panel write), §13 non-goals. Pass.

**Nonblocking doc corrections (recommended, do not expand scope)**

1. §9, last sentence: "它们必须明确报告 authority unchanged" contradicts "AppSDK init … 不做代码修改". This is the editorial inconsistency you flagged. Delete the output requirement (or reduce it to the invariant "init must not clear authority"), keeping the no-code-change decision. Do not add an init output surface.
2. §12 / plan P03: `cargo test --manifest-path collab/Cargo.toml --lib` has no lib target (package has no lib targets). Use `--bin collab` (base: 925 passed / 0 failed / 1 ignored). Purely a command correction; the contract defers commands to P01/P03, so fix it in the plan and, if echoed, in the contract.

**Optional (nonblocking, no action required)**

- §7.1 names the holder field `worker_id`, whereas the v1 review draft used `holder_worker_id`. The design is the authority, so either is fine; keep one name across status/context/board/panel when implementing.

**Scope confirmation**

I did not run tests, fixtures, services, installs, commits, or agents; made no edits and created no root notes/result files. AppSDK init scope remains untouched in the design (no product expansion). The design admission gate is satisfied; the later gates (implementation, debug, dev tests, E2E, installed candidate, and the separate final architecture review) remain as planned.