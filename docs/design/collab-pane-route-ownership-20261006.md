# Pane route ownership: one pane, one binding

Status: design revision 7. Supersedes the L1 half of
`collab-reset-operation-20261005.md` and the scope-local decision recorded in
`collab-control-plane-reset-run-notes.md` (2026-10-05 17:15).

Revision 2 removed the three-level split's L1 and added a host-wide eviction.
Revision 3 removed revision 2's liveness gate, which the second review proved
cannot resolve across projects. Revision 4 answers the third review, which
confirmed that no republisher path evicts once the guard lands, and found that
revision 3 mis-named the writer and republisher call sites, mis-stated the
replay counts, and named two different owners for the replay fallback. Revision
5 answers the fourth review: the writer and republisher inventories are exact,
and the replay counts are re-derived with the correct address function, so
review 5 returned PASS.

Revision 6 records what implementation changed against revision 5. Two points
moved: the republisher predicate must also exempt this binding's own stale
route at an older address, and the replay fallback keeps its
session-and-thread ambiguity error while the pane case goes to the reducer.
Both were found by the rewritten contract tests, not by review. Sections
changed since revision 3 are marked `(revision N, review M Pn)`; sections
changed by implementation are marked `(Revision 6, …)`.

Revision 7 answers the sixth review. It found the "never reaches the reducer"
absolute in section 5 false: `collab context --worker <owner>` retires the stale
cross-project anchor through the pre-existing adjudication channel, so a
stranded pane is releasable in band. It also found that the deleted in-scope
pane query used to provide a scope check that the host-wide
`MASTER_RECOVERY_BLOCKED_LIVE` gate no longer made, which is restored. Sections
changed here are marked `(Revision 7, review 6 Pn)`.

## 1. The defect

One tmux pane can hold two live route claims at the same time. The host index
proves it, and so do two shipped tests.

Measured on the live host index
(`<storage_root>/.agent-collab/server/journal.jsonl`, 24297 lines at
2026-10-05T19:10-07:00, 122 `GlobalCurrentThreadRouteSet` events, 30 live
routes):

| pane | claimants | scope relation |
|---|---|---|
| `$2:%2` | appsdk `binding-codex-_2` + routecodex `binding-codex-01a0cd77-…` | cross-project |
| `$138:%138` | macjev `binding-codex-_138` + camo `binding-codex-_138` | cross-project |
| `$16:%16` | OneStop `binding-codex-_16` + OneStop `binding-collab-master-status` | same project |
| `$6:%6` | routecodex `binding-codex-_6` + routecodex `binding-01a0d7c0-…` | same project |
| `$8:%8` | humanagent `binding-codex-_8` + humanagent `binding-codex-01a0c787-…` | same project |

The shipped tests encode the overlap as intended behaviour:

- `cross_scope_pane_claimant_does_not_fence_the_project_route`
  (`host_route_registry_tests/part_02_part2.rs`) asserts
  `tmux_pane_route_claimants(&endpoint).len() == 2` with the message "the pane
  is shared host-wide".
- `superseded_same_pane_master_does_not_fence_project_route` states "Both
  bindings now share the pane in the host index".
- `runtime_manager_setup.rs:260-263` (pre-change; this delivery deletes the
  comment) stated "A pane is addressable by exactly one live peer per project".

Both sides of every pair are ordinary bindings with full codex anchors
(`session_id == native_thread_id == codex_session_id == codex_thread_id`). The
`binding-codex-_<pane>` naming reflects the pane, not a weaker binding kind.
*(Revision 3, review 2 P2: revision 2 claimed the evicted side was a synthetic
pane-self claim. That claim was false.)*

### Root cause

`current_thread_routes` is keyed by the route address, and
`current_thread_route_address` (`global_state_helpers.rs:9-25`) returns
`(session_id, native_thread_id)` whenever the endpoint carries codex anchors.
The pane is only a query dimension. Two bindings with different threads
therefore get two keys and coexist. `same_scope_pane_owner_supersedes`
(`part_04.rs:21`) only stops the older claim from fencing; it never removes it,
so the resource-level overlap stays.

## 2. The target invariant

**For every tmux pane, `current_thread_routes` holds at most one binding.**

A `GlobalCurrentThreadRouteSet` on a pane already owned evicts the earlier
claimant, whatever its project or route scope. A binding with no tmux endpoint
is unconstrained: it shares no resource, and the eviction predicate must never
drop it.

## 3. Single owner and the exact predicate

`GlobalState::set_current_thread_route`
(`server/global_state_impl_part2.rs`) owns the invariant. The eviction happens
inside its `mutate` closure, before the insert. `set_current_thread_route` is
the only writer of `current_thread_routes` on both the live and the replay path
(the five funnel sites are `state_impl.rs:747-767`, `part_04.rs:500`,
`part_06.rs:958`, `part_02.rs:312`, and the identity).

The predicate is pane identity, not raw endpoint equality:

```text
let pane = binding.tmux_endpoint.as_ref();
next.current_thread_routes
    .retain(|_, existing| match (pane, existing.tmux_endpoint.as_ref()) {
        (Some(a), Some(b)) => !same_pane_route(a, b),
        _ => true,
    });
next.current_thread_routes.insert(route_address, binding);
```

`same_pane_route` (`adapters/tmux.rs:308-314`) compares socket path, server pid,
tmux session id, pane id and pane pid. Raw endpoint equality is wrong twice
over: it would drop every entry whose `tmux_endpoint` is `None`, and it would
miss a pane whose `server_pid` changed after a tmux server restart.

## 4. Why the winner is the record

The winning binding is itself the durable record. `current_thread_routes` is
rebuilt from the journal on every start, so "pane P is owned by binding X" is
durable, auditable and replayable with no extra state.

A separate `evicted_route_claims` map would be worse: it would lock the pane
away from the evicted project even after the winner has left, because the
evicted address would stay marked.

## 5. Writer paths and republisher paths, with no liveness probe

*(Revision 3, review 2 P0. Revision 2 gated the republisher on the owner being
live. That cannot work and is removed.)*

Revision 2 proposed widening the liveness probe to host-wide. The review proved
this is impossible. `same_scope_pane_owner_supersedes` (`part_04.rs:32,44-60`
before this delivery deleted it) resolves the owner through
`runtime.state.workers`, and then through the host
server's worker table, both keyed by `route.agent_id`. The journal holds 7
`Registered` events, all appsdk, while 122 route sets come from at least 10
projects. A project's workers never enter the host table. Two failures follow:

- **Starvation.** `$138:%138` is claimed by camo and macjev under the *same*
  agent id `codex-%138`, so a local same-id worker makes a dead cross-project
  owner look live and the pane is never released.
- **Oscillation.** With no local id match, a live cross-project owner looks
  dead, so a republisher evicts it. At the first restart
  `reconcile_started_thread_routes` would republish appsdk `binding-codex-_2`
  and evict the routecodex master on `$2:%2`.

The guard therefore uses no liveness at all. It is a pure read of the index.

**Writer paths — a live act of registration. They commit
`Event::GlobalCurrentThreadRouteSet` directly, evict, and are never guarded.**
The later writer wins by definition. There are exactly three:

- `commit_current_thread_route_for_runtime` (`part_06.rs:901-960`, committing at
  `:958`). A successful registration is live activity.
- `finalize_registration` (`part_04.rs:549`, calling `commit_current_thread_route`
  at `:563`, which commits at `:500`).
- `typed_dispatch` (`part_02.rs:312`).

**Republisher paths — recovery restores a route after a restart. They never
evict.** Before publishing binding `B` on pane `P`:

1. Does `P` hold a claim that is not `B`'s own route? If no → publish.
2. If yes → skip, and append
   `RECOVERY_RECONCILE_SKIPPED_SUPERSEDED: <B> lost pane <P> to <O>` to the
   host log.

Two exclusions in step 1 are load bearing, and both are needed:

- A claimant at `B`'s own route address is `B` itself. A generation refresh
  keeps the same address, so it must still publish, or the index keeps a stale
  `endpoint_generation` and `same_pane_master_route_ready` fences the project.
  *(Revision 4, review 3 P1.)*
- A claimant that is the same durable binding at a *different* address is `B`'s
  own stale route, not another owner. A registration whose route commit fails
  leaves the older generation live at the older address while the project's
  durable binding has already advanced. Treating that as another owner would
  hand the pane away and strand the project. The repair is to advance `B`'s own
  route, which is exactly what publishing does.
  *(Revision 6, found by the contract tests during implementation.)*

The republisher call sites are `reconcile_same_pane_master_routes` and
`reconcile_started_thread_routes` in `runtime_manager_setup.rs`. Every one of
their commits goes through the guard first, and each commit is
`Server::commit_checked` with `Event::GlobalCurrentThreadRouteSet`. A guard on
only one of the two functions would leave the other's commits exposed.
*(Revision 5, review 4 P2.)*

The request-path fence `same_pane_master_route_ready`
(`runtime_manager_setup.rs:251-295`) applies the same predicate: a pending
master whose pane is held by another claim is resolved, so it does not fence
the project; a pending master whose pane is held by nobody fails closed with
`RECOVERY_RECONCILE_REQUIRED`.

Without its guard, `reconcile_started_thread_routes` republishes at every
startup and evicts the winner.

The replay fallback `restore_unique_current_thread_routes_from_bindings`
(`state_impl.rs:3-72`) is the one exception to this list. It is a `State`
method called from `replay_from_journal` (`part_12.rs:507`) before any `Server`
exists, so it cannot call the guard. Its owner is the reducer itself; see
section 6.5.

### Single owner for the guard

The predicate has one owner and the decision has one owner, so the four
republisher call sites and the request fence cannot drift:

- `GlobalState::pane_claimant_other_than(&self, binding) -> Option<&RuntimeBinding>`
  answers "which claim owns this pane other than this binding's own route".
  There is no second way to ask.
- `ProjectRuntimeManager::pane_owner_other_than(&self, binding) -> Option<RuntimeBinding>`
  is the decision: it locks the host state and applies the predicate. Every
  republisher call site and the request fence go through it.

Each site writes its own log line, because the two reconcilers and the request
fence report different things: the reconcilers report a skip, and the fence
reports that it did not fence. The publish itself stays at each site, because
`reconcile_same_pane_master_routes` must inspect the current pane holder's
generation before it decides, and `reconcile_started_thread_routes` must
inspect the current route at its own address.

The three writer paths are not guarded, because a registration is the later
writer by definition. The replay fallback of section 6.5 is handled by the
reducer instead, for the reason given there.

This gives both properties:

- **No oscillation.** A republisher never evicts. Only a new registration can
  take a claimed pane.
- **No cross-project knowledge.** The guard reads only the local index.

### Accepted consequence: a stale claim holds the pane

If an owner stops without releasing its route, the pane stays claimed and no
republisher takes it. This is deliberate and is the trade the invariant makes.
The pane is released by one of two events:

1. The owner removes its route as part of its own lifecycle
   (`Event::GlobalCurrentThreadRouteRetired`, emitted by the launch-failure
   cleanup in `part_06.rs` and by the registration rollback in `state_impl.rs`).
2. A writer commits a new route on the pane. A registration is a writer path, so
   the reducer evicts the stale claim and the new writer wins.

Case 2 has two doors, and the difference is whether the operator names the
worker.

- **Without `--worker`**, a cross-project registration on a pane whose stale
  claim is still in the index never reaches the reducer: the pre-existing
  identity guard refuses it first with `RUNTIME_BINDING_REJECTED: tmux identity
  anchor is already bound to worker <owner>`. The merged-main binary and this
  candidate emit that refusal identically, so the invariant neither causes nor
  fixes it. *(Revision 6, measured by the isolated black box in
  `/tmp/collab-pane-bb/stale-claim.log`.)*
- **With `collab context --worker <owner>`**, the pre-existing adjudication
  channel sets `retire_cross_project_anchor`, so the guard does not refuse: it
  retires the stale cross-project anchor
  (`retire_cross_project_anchor_candidate`, `runtime_manager_setup.rs:619`) and
  the registration then reaches the reducer, which evicts the same-pane claim. A
  stranded pane is therefore releasable in band, through an operator channel
  that predates this delivery. *(Revision 7, review 6 P2-1; pinned by
  `named_override_retires_a_stale_cross_scope_anchor`.)*

What case 2 covers without an operator override is a writer inside the same
anchor: a generation refresh, or a re-registration once the old route is gone.
And the reducer contract holds either way: whenever an accepted writer installs
a route on the pane, the older claimant is gone instead of co-resident.

A cross-project stranded pane therefore needs either the owner's own route
retirement or `collab context --worker <owner>`. A plain registration is refused
before the reducer runs. That boundary is pre-existing and is reported, not
hidden.

## 6. Ablation

### 6.1 Level 1 of `collab reset`

`ResetLevel::Routes`, the `--routes` flag, the `--keep` flag,
`pane_claimant_groups`, `describe_pane`, `RESET_KEEP_REQUIRED`, the `--routes`
arm of `RESET_STORAGE_ROOT_REQUIRED`, `run_routes`, and `verify_retirement`.
Once the reducer enforces the invariant, `pane_claimant_groups` can only return
single-element groups, so `RESET_KEEP_REQUIRED` is unreachable. `collab reset`
keeps `--project` and `--host`.

### 6.2 The operator retirement mechanism, and the pane-conflict errors

The retirement mechanism's only producer was L1 (`reset.rs:1117` builds
`GlobalRouteClaimRetired`; the live journal holds zero such events). Delete
`Event::GlobalRouteClaimRetired`, `GlobalState.retired_route_claims`,
`RetiredRouteClaim`, `record_retired_route_claim`,
`lookup_retired_route_claim`, `retired_route_claim_key`, and
`state_impl.rs:1096-1108`'s snapshot emission.

Removing the variant is a journal-format change, and the consequence is stated
rather than hidden: `decode_journal_line` (`part_12.rs`) rejects an unknown `ev`
tag with no catch-all, so a journal that already holds a
`GlobalRouteClaimRetired` record no longer replays and the daemon refuses to
start loudly instead of starting with silent data loss. The live host journal
holds zero such records (measured over 122 route sets), and the level that wrote
them shipped only in the immediately preceding delivery, so no host is expected
to carry one. If one does, `collab reset --host` archives the journal and
rebuilds the baseline. A decode-only shim is deliberately not kept: the
variant's producer is gone, and a permanent ignore-arm for a removed event is
the dead path this ablation removes.
*(Revision 7, both sixth-review passes.)*

This settles review 2's P1. The pane-conflict `RECOVERY_RECONCILE_REQUIRED`
returns at `runtime_manager_setup.rs:322`, `:337` and `:414` are exactly what a
cross-project owner reaches, and their remedy text tells the operator to run
`collab reset --routes --keep <binding_id>`, a command section 6.1 deletes.
They are replaced by the section 5 skip-and-log path, so no error survives that
names a deleted command. A generation-mismatch error that is not a pane
conflict stays, and its remedy text is rewritten to name the registration path
instead.

The retired-claim branches themselves are **three**, at
`runtime_manager_setup.rs:277`, `:374` and `:475`. `:277` is the fence. They
are deleted with the mechanism, not replaced by the section 5 guard.
*(Revision 3, review 2 P3: revision 2 said four.)*

Review 2's P1-3 also settles `state_impl.rs:40-46`: it runs while rebuilding
from project bindings, before any route exists, so no "different claimant"
check can replace it. It is deleted with the mechanism it serves. There is no
operator retirement left to resurrect.

### 6.3 The scope-local pane queries

`tmux_pane_route_claimants_in_scope` and
`lookup_unique_tmux_pane_route_in_scope` were introduced to work around the
overlap. They go, at all five production call sites:

| call site | replacement |
|---|---|
| `retry_pane_registration` (`part_04.rs`) | `lookup_unique_tmux_pane_route` |
| `resolve_staged_pane_recovery` (`part_04.rs`) | `lookup_unique_tmux_pane_route` |
| `handle_register_with_app_scope_inner` (`part_06.rs`) | `lookup_unique_tmux_pane_route` |
| `validate_cli_register_rebind` (`part_10.rs`) | `lookup_unique_tmux_pane_route` |
| `same_pane_master_route_ready` (`runtime_manager_setup.rs`) | `pane_owner_other_than` and `lookup_unique_tmux_pane_route` |

`retry_pane_registration` and `resolve_staged_pane_recovery` already paired the
host-wide lookup with a same-worker, same-pane identity check on the result.
Those checks are load-bearing and stay: they turn a host-wide pane query into
"this pane is this worker's pane", which is what a retry needs.

### 6.4 The supersede probe

`same_scope_pane_owner_supersedes` (`part_04.rs:21-60`) is **deleted**, not
renamed. Revision 2 kept it; section 5 removes the need for any liveness probe.

### 6.5 The replay fallback

`restore_unique_current_thread_routes_from_bindings` (`state_impl.rs:3-72`)
sorts bindings by `native_thread_id` and
`return Err("journal replay rejected ambiguous current thread route …")` on the
first conflict. With the reducer enforcing the invariant, a pane conflict
becomes unreachable, and the sort order becomes a second ordering source.

The function stays as the fallback that rebuilds routes from bindings. Its owner
is **not** the section 5 guard: the function is a `State` method called from
`replay_from_journal` (`part_12.rs:507`) before any `Server` exists, so it
cannot call `pane_owner_other_than`. Its owner is the reducer.

The conflict branch is narrowed rather than deleted, because a *session-and-thread*
collision is still a real error that only this function can see: 73 live routes
have no tmux endpoint and key on `(session_id, native_thread_id)`, so two
projects can rebuild the same address with no pane involved. The function
therefore keeps the error for that case, and defers the pane case to the
reducer: a binding that has a tmux endpoint is passed straight to
`set_current_thread_route`, which evicts the same-pane claimant. The sort order
is the section 8 fallback order.
*(Revision 6: revision 5 said the branch is deleted. Implementation showed that
deleting it also removes the address collision check that `state_tests.rs`'s
`runtime_binding_replay_rejects_ambiguous_current_thread_routes` covers.)*

### 6.6 Governance artifacts

**`docs/dagpipe/collab-control-plane-reset.graph.json`** — 0.2.1 with 8 nodes.
`verify_retirement` is the only node to remove. The chain
`retire_selected_state → verify_retirement → rebuild_baseline` becomes the
single arc `retire_selected_state → rebuild_baseline`. The description loses
the routes level, `pane_claimant_groups`, `RESET_KEEP_REQUIRED` and
`RESET_VERIFY_FAILED`, and its terminal list is re-derived from `reset.rs`.
Version bumps to 0.3.0.

**`docs/dagpipe/collab-pane-route-reconcile.graph.json`** — 0.3.0 with 5 nodes.
It is rewritten, not deleted, because the reconcilers and the fence survive.
`resolve_scope_pane_owner` becomes `resolve_pane_owner` and
`verify_scope_pane_uniqueness` becomes `verify_pane_uniqueness`, with their
`arc_id`s and output ids renamed to match; both take host-wide, no-liveness
semantics. The node count stays 5. Version bumps to 0.4.0. The uniqueness check
sits *before* the publish, because the guard is what decides whether to publish
at all: the graph must not claim a post-publish verification the code does not
perform. *(Revision 7, review 6 P2-7.)*

**`rust/src/dagpipe.rs`** — `design_graph_ids()` stays `[&'static str; 11]` and
`design_graph_operator_names()` goes from `[&'static str; 64]` to
`[&'static str; 63]`: the two pane-route names are renamed in place and
`appsdk.collab_control_plane.verify_retirement` is removed. The array is
recomputed from the graphs, not edited by hand. The gate is `cargo test dagpipe`
in `rust/`; `appsdk dagpipe validate` does **not** run the registry or compile
check and must not be used as the gate.

**`docs/design/collab-reset-operation-20261005.md`** — the L1 sections, the
`--keep` contract, the retirement-record sections and the failure-terminal
table lose their L1 rows. `docs/design/collab-control-plane-reset-run-notes.md`
gains a note that the scope-local decision of 17:15 is reversed here.

**`docs/design/collab-control-plane-reset-20261005.md`** — the delivery-1 design
loses its routes-level rows, tables and DAG entries and is reframed as a
two-level document. Its historical review notes stay, each marked as naming a
removed level.

**`collab/skills/collab/SKILL.md`** and
**`collab/skills/collab/references/state-paths.md`** — the shipped operator
description of the reset levels loses the `--routes`/`--keep` surface, so the
installed skill bundle matches the two-level CLI.

`docs/dagpipe/manifest.json` carries only id and path, so it needs no version
or operator change. *(Revision 3: revision 2 wrongly listed it.)*

### 6.7 Tests

Deleted with their subjects:
`reset_routes_retires_each_ambiguous_pane_and_keeps_the_named_survivor`
(`reset_tests.rs:937`), `reset_routes_refuses_an_unnamed_pane_without_touching_the_journal`
(`:1043`), `verify_retirement_requires_the_retirement_and_a_live_survivor`
(`:1135`), `reset_routes_does_not_group_a_distinct_pane_with_the_same_session_and_pane_id`
(`:1351`), `reset_routes_records_the_full_pane_identity_per_retired_claim`
(`:1423`), the retirement tests in
`host_route_registry_tests/part_02_tail.rs`, and the now-unused helpers
`route_endpoint`, `route_endpoint_fields`, `route_binding`, `route_binding_at`.
`reset_level_requires_exactly_one_selector` and
`reset_level_flags_are_gated_per_level` are rewritten for two levels.

## 7. Contract changes

The two tests that encoded the overlap are rewritten to the new contract, not
deleted. Each keeps its scenario and asserts the new outcome. Three further
tests encoded the same contract from the other side and are inverted with it.

| test | old assertion | new assertion |
|---|---|---|
| `superseded_same_pane_master_does_not_fence_project_route` (`part_02_part2.rs`) | both bindings share the pane | the older claimant is evicted; after reconcile the pane has exactly 1 claimant |
| `cross_scope_pane_claimant_does_not_fence_the_project_route` → `cross_scope_pane_claimant_takes_the_pane_without_fencing_the_project_route` | pane shared host-wide, 2 claimants, cross-project is not a conflict | the cross-project claimant owns the pane alone, the master address is gone, and the project route still resolves |
| `same_pane_peer_that_moved_away_does_not_supersede_the_master_anchor` → `same_pane_peer_that_moved_away_still_owns_the_pane` (`part_02_tail.rs`) | a peer that left the pane stops being an owner, so the master is not fenced | ownership is an index fact, not liveness: the peer still owns the pane and the master is fenced |
| `closed_same_pane_peer_does_not_supersede_the_master_anchor` → `closed_same_pane_peer_keeps_the_pane_ownership` (`part_02_part2.rs`) | a dead peer frees its pane | the peer still owns the pane, and the claimant count stays 1 |

`same_scope_pane_claimants_are_named_in_the_fence_error` is **deleted**: it
asserted that a fence error names the same-scope claimant, and a pane now has at
most one claimant. `same_pane_master_still_fences_when_host_route_is_missing` is
extended instead: it now asserts the real
`RECOVERY_RECONCILE_REQUIRED: … re-register the worker` fence before the peer
publishes.

The doc comments in `part_02_part2.rs`, `part_02_tail.rs` and
`runtime_manager_setup.rs` are corrected to "one pane owns exactly one binding,
host-wide".

## 8. Resolution order for the existing duplicates

The rule is the one the objective names: the later writer in the journal wins.

> The winner is the binding whose `GlobalCurrentThreadRouteSet` is applied last.
> Live writes and journal replay apply events in journal order. The fallback of
> section 6.5 applies bindings in `native_thread_id` sort order.

A read-only replay of the live journal under this rule gives 122 route-set
applications and **8 evictions**. The strict index `current_thread_routes`
holds **102 route addresses before the change and 97 after**: 29 with a tmux
endpoint before, 24 after, and 73 endpoint-less either way. No pane keeps two
claimants.

The 102 route sets in journal lines 358-459 are that index, re-emitted by one
compaction snapshot. All 102 carry a `session_id`, so they are strict routes and
not `legacy_thread_routes`, which is empty here because
`set_legacy_thread_route` rejects any binding that has a session id.

Following the address function matters when counting. A binding with **no**
tmux endpoint keys on `(session_id, native_thread_id)`, while a binding whose
endpoint carries no codex anchor keys on the five tmux fields
(`global_state_helpers.rs:10-25`). Treating the absent endpoint as a pane
address collapses the 73 endpoint-less routes into one and reports a false
index size. *(Revision 5, review 4 P1: revision 4 reported 30 → 25 from exactly
that error.)*

| pane | evicted | winner |
|---|---|---|
| `$2:%2` | appsdk `binding-codex-_2` | routecodex `binding-codex-01a0cd77-…` gen22 |
| `$138:%138` | macjev `binding-codex-_138` gen2 | camo `binding-codex-_138` gen1 |
| `$16:%16` | OneStop `binding-codex-_16` gen1 | OneStop `binding-collab-master-status` gen1 |
| `$6:%6` | routecodex `binding-codex-_6` gen8 | routecodex `binding-01a0d7c0-…` gen3 |
| `$8:%8` | humanagent `binding-codex-_8` gen4 | humanagent `binding-codex-01a0c787-…` gen7 |

All five panes are decided twice during the replay, because the compaction
snapshot lists both claimants for each. So the winner follows the last
emission, which for a snapshot is the address `BTreeMap` order. This is
deterministic but not meaningful: the two sides of every pair are equally
legitimate bindings, `endpoint_generation` is a per-binding counter and is not
comparable across bindings, and no durable per-claim timestamp exists.

**Residual risk, stated openly.** The order decides each pane exactly once,
because once the invariant holds the index has at most one claimant per pane
and a re-emission cannot flip a winner. But the first post-change replay is
what freezes the outcome, so the acceptance run must confirm that the routecodex
master keeps `$2:%2`. The only `GlobalMasterGranted` in the journal is appsdk
`binding-codex-_3` on `$3:%3`, which is not in the eviction set. Measured per
project, only the five panes change: appsdk 7 → 6 routes, OneStop 14 → 13,
humanagent 25 → 24, macjev 4 → 3, routecodex 27 → 26; the other six projects
are unchanged.

## 9. Failure terminals and observability

The invariant removes an error class instead of adding one. There is no
`PANE_ROUTE_OWNERSHIP_AMBIGUOUS`, because the reducer cannot leave two
claimants at one pane. The pane-conflict `RECOVERY_RECONCILE_REQUIRED` returns
of section 6.2 are replaced by a skip, and no terminal names the deleted
`--routes --keep`. The new observable is:

- `RECOVERY_RECONCILE_SKIPPED_SUPERSEDED: <binding> lost pane <pane> to <owner>`
  on the host log, for each skipped republish.

## 10. Acceptance

- **Red first.** Neuter the eviction in `set_current_thread_route` and show the
  two rewritten contract tests fail. Restore it and show them pass.
- **Targeted.** `cargo test --all-targets` in `collab/`; `cargo test dagpipe` in
  `rust/` after the graph and registry updates.
- **Black box, isolated state root.** Two projects claim one pane. After replay
  the pane has exactly one claimant. Then run **three** `collab down`/`collab
  up` cycles and assert the claimant set and the route count are unchanged
  after each cycle, and that the journal does not grow by a route set per
  cycle.
- **Stale-claim release.** A plain cross-project registration onto a pane whose
  stale claim is still in the index is refused before the reducer runs, by the
  pre-existing identity guard. That refusal is measured identically on the
  merged-main binary and on this candidate
  (`RUNTIME_BINDING_REJECTED: tmux identity anchor is already bound to worker
  <owner>`, `/tmp/collab-pane-bb/stale-claim.log`). The in-band release is the
  pre-existing adjudication channel: `collab context --worker <owner>` retires
  the stale cross-project anchor and the registration then reaches the reducer
  (pinned by `named_override_retires_a_stale_cross_scope_anchor`). At the reducer
  itself the acceptance is direct: the fixture replay shows the earlier claimant
  absent from the index once the later one is installed, and
  `closed_same_pane_peer_keeps_the_pane_ownership` and
  `same_pane_peer_that_moved_away_still_owns_the_pane` pin the index behaviour.
- **Fence scope.** The `MASTER_RECOVERY_BLOCKED_LIVE` exemption
  (`same_pane_tmux_recovery`) uses the host-wide pane query, so it re-applies the
  scope check the deleted in-scope query used to provide: the found route must
  match the requesting worker's project and app scope. Without it, a foreign
  route that reuses the pane-derived `binding-codex-%N` name would skip the
  fence.
- **Live index.** A read-only replay of the live host journal under the delivered
  rule reports the same 8 evictions and the same five winners as section 8, and
  the running daemon was then probed one address at a time with the read-only
  `collab route resolve`. All five panes agree: the winner is in the index and
  the evicted claimant is not. `$2:%2` resolves to routecodex
  `binding-codex-01a0cd77-…` gen22 while appsdk `binding-codex-_2` returns
  `ROUTE_RESOLVE_NOT_FOUND`; `$6:%6` resolves to routecodex
  `binding-01a0d7c0-…` gen3 and `$16:%16` to OneStop
  `binding-collab-master-status`, with their evicted claims absent. The `%8` and
  `%138` panes no longer exist, so their winner's address answers
  `tmux endpoint does not match the current route for pane …`, which
  `part_04.rs:321-326` emits only after the strict address lookup has already
  succeeded, while their evicted addresses answer "no registered Collab route is
  bound to …". The real-entry window confirms the decisive consequence:
  routecodex `collab context` reports the master live with no `exact_error`.
- **Real entry.** routecodex `collab context` stays clean across a down/up
  window, and the host log gains no new `RECOVERY_RECONCILE_REQUIRED` or
  `MASTER_RECOVERY_BLOCKED_LIVE` line.
- Independent architecture review, then merge and push.

## 11. Revision: a pane is a resource, not a liveness credential

Section 5 and the stale-claim acceptance in section 10 describe the state after
the first delivery. This section supersedes them. The owner set the contract in
three sentences:

1. The identity is fixed at registration. A tmux registration anchors on its
   pane, and a dsh registration anchors on its session and thread. Every
   identity has one anchor that can always decide it.
2. For tmux, only the pane id decides. The same pane id anchors the new
   registrant directly. A registration on another pane does not transfer a
   grant; only the owner's authorization strips the old grant.
3. Any authorized promotion replaces the incumbent. The old grant is no veto.

### 11.1 The delivered rule

A tmux pane is a resource with one owner. When a worker registers on a pane that
another binding already holds, the daemon accepts the later registrant and
replaces the old binding. The replacement covers another project and another
route scope. It needs no retire flag, and no old master and no old claimant can
refuse it. The previous claimant loses the pane, its current-thread route and
its worker record.

The Codex session and thread a tmux endpoint carries stay a conflict fence, not
an anchor. Another worker that holds that thread on a *different* pane is still
refused, because the pane is the only thing that changed owner. The refusal
happens before any commit, so the journal does not move.

A tmux-only binding has no queryable AppServer session or thread. Its liveness
is therefore `Missing` unless a later registration re-anchors it. The pane
probe answers addressability, and it is the anchor for the tmux identity. It
never makes a binding live on its own.

### 11.2 Code changes

- `Req::Register.retire_cross_project_anchor` is deleted, with its
  `main_context.rs` producer and its thread-local flag in `main.rs`. The flag
  existed only to authorize the replacement this revision makes unconditional.
- `validate_cli_register_rebind` no longer fences pane ownership. The typed
  registration path owns that decision.
- `validate_current_thread_candidate` matches the anchor on the pane only
  (socket, tmux session, pane id). The Codex ids are used only for the conflict
  fence above. A claimant that lives in the runtime this registration commits
  into is left to that commit, so the replacement is one transaction. A claimant
  in another runtime is retired there, because only that runtime's global state
  knows the claimant's project scope.
- `pane_claimants` and `pane_reclaim_events` (`part_06.rs`) are the single
  builder for a replacement. `typed_dispatch` calls it inside the registration
  commit; `validate_current_thread_candidate` calls it for a foreign runtime.
- `handle_master_promote` no longer vetoes an incumbent. An explicit approval is
  the whole authority for the transition, so the grant moves to the named
  worker.

### 11.3 Delivery gates, unchanged

The revision does not weaken delivery. An ordinary send still blocks on
`Missing` only. `daemon_to_peer` and `restart_replay` still require `Present`.
A cross-project send still requires `Present` on both sides, and `collab master
send` still requires `master.endpoint_live == true`. The stale-master symptom is
fixed at the root: the pane re-anchors to the later registrant, and an
authorized promotion always replaces the incumbent.

### 11.4 Tests

- `a_second_claim_on_the_same_pane_replaces_the_first_claimant` (new): a second
  registration on one pane wins it, and the previous claimant is closed.
- `a_codex_thread_on_another_pane_stays_a_conflict` (new): the same thread on
  another pane is refused, and the journal does not move.
- `a_later_registration_takes_a_foreign_scope_anchor` (renamed from
  `named_override_retires_a_stale_cross_scope_anchor`): a plain registration
  takes a foreign-scope anchor, with no operator override.
- `approved_promotion_replaces_the_incumbent_without_a_pane_taker` and
  `approved_promotion_replaces_the_incumbent_even_after_the_peer_moved_on`
  (renamed): an authorized promotion replaces the incumbent.
- `approved_promotion_supersedes_a_live_tmux_master`,
  `user_approved_promotion_replaces_a_master_that_owns_its_anchor`,
  `user_approved_promotion_replaces_a_master_after_its_pane_is_taken`
  (renamed): the incumbent grant is not a veto.
- `wire_cli_recover_rejects_a_forged_token_and_a_forged_route` (renamed): a
  forged token is still refused; a claim on another worker's pane is accepted
  and closes that worker.
- `closed_same_pane_peer_keeps_the_pane_ownership` (rewritten): the later
  registrant closes the recorded master, and the closed peer keeps its durable
  host route, so the pane stays owned.

