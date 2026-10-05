# Collab control-plane reset and pane-route uniqueness (2026-10-05)

Status: revision 2 after independent design review (round 1 returned FAIL on
revision 1; every P0/P1 is answered below). Product code starts only after the
frozen revision passes review.

Revision 1 proposed retiring every same-pane claimant when a route is installed.
Review P0-1/P0-2 rejected it: the contract is per-project pane ownership, and an
implicit retirement of a live conflict contradicts the recovery DAG. Revision 2
replaces it with a scope-local query and moves retirement to an explicit,
offline operator operation.

## 1. Proven problem

The routecodex master `codex-01a0cd77-8c40-7a32-85a8-f5d191261b8d` cannot
recover. The daemon answers every non-`Register` request with
`RECOVERY_RECONCILE_REQUIRED: host route for <worker> is not at project
generation 22`, and every register-based recovery attempt with
`MASTER_RECOVERY_BLOCKED_LIVE`. Reproduced at the real entry on 2026-10-05 17:09:
`collab context` in `/Users/fanzhang/Documents/github/routecodex` returns that
exact error with `"registered": false`.

Measured evidence (state of 2026-10-05):

| Fact | Value |
| --- | --- |
| fence rejections | 282, 2026-09-29 05:03 -> 2026-10-05 01:45 |
| rejected ops | Context 86, StatusAll 46, MsgStatus 46, MasterStatus 31, Send 20, Workers 13, WorkerStatus 13, Inbox 13, TaskStatus 6, MigrationInspect 3 |
| register rejections | `MASTER_RECOVERY_BLOCKED_LIVE`, >= 7 attempts 2026-10-03 -> 10-04 |
| host log | 343 identical fence errors; daemon restarts 375 up / 323 down requested |

Replay of the daemon control-plane journal
(`<storage-root>/.agent-collab/server/journal.jsonl`, 7869 events, 111 route
sets, **0 retirements**, 102 live routes) gives the root cause. Pane `$2:%2` has
two route claimants:

| agent | binding | generation | codex session/thread | scope |
| --- | --- | --- | --- | --- |
| `codex-01a0cd77-...` (master) | `binding-codex-01a0cd77-...` | 22 | `01a0cd77` | routecodex |
| `codex-%2` (stale) | `binding-codex-_2` | 1 | `01a0e5be` | appsdk |

`lookup_unique_tmux_pane_route`
(`collab/src/server/global_state_impl.rs:539-550`) returns a route only when
exactly one binding claims the pane, and it scans **every** route regardless of
project scope. Two claimants therefore yield `None`, and the fence compares
`None != Some(binding)`
(`collab/src/server/mod_parts/runtime_manager_setup.rs:263`). The reconciler
cannot repair it: `set_current_thread_route`
(`collab/src/server/global_state_impl.rs:727-741`) retires only entries with the
same route scope and binding id, and the `None` branch
(`runtime_manager_setup.rs:292-298`) only republishes the requester. So every
repair round leaves the pane ambiguous, which is why the same generation 22 was
committed three times and the error repeated 282 times.

The same global lookup causes the second failure mode. At
`collab/src/server/mod_parts/part_06.rs:1144` the register path decides
`same_pane_tmux_recovery` with the global lookup. A cross-scope claimant makes
that lookup return `None`, so the flag is false, the request falls through to the
live-master guard at `part_06.rs:1165-1179`, and the answer is
`MASTER_RECOVERY_BLOCKED_LIVE`. One wrong query produces both failures.

Pane distribution of the current index (read-only replay, grouped by
`(route_scope, pane)`):

| pane | claimants | class |
| --- | --- | --- |
| `$2:%2` | master (routecodex, gen 22) + `codex-%2` (appsdk, gen 1) | cross-scope |
| `$138:%138` | `codex-%138` (camo, gen 1) + `codex-%138` (macjev, gen 2) | cross-scope |
| `$8:%8` | `codex-01a0c787` (humanagent, gen 7) + `codex-%8` (humanagent, gen 4) | same scope |
| `$6:%6` | `codex-%6` (routecodex, gen 8) + `01a0d7c0` (routecodex, gen 3) | same scope |
| `$16:%16` | `codex-%16` (OneStop, gen 1) + `collab-master-status` (OneStop, gen 1) | same scope, generation tie |

The codebase already states the intended invariant. The comment at
`collab/src/server/mod_parts/runtime_manager_setup.rs:248` says "A pane is
addressable by exactly one live peer **per project**";
`same_scope_pane_owner_supersedes`
(`collab/src/server/mod_parts/part_04.rs:21-45`) filters by
`other.route_scope() == binding.route_scope()`;
`validate_current_thread_candidate` (`runtime_manager_setup.rs:498-507`) only
uses a pane as an anchor when both Codex ids are absent and notes that "a shared
pane cannot block a second App Server peer"; and the test
`superseded_same_pane_master_does_not_fence_project_route`
(`collab/src/server/host_route_registry_tests/part_02.rs:1055`) asserts that two
same-scope claimants may coexist in the host index and must not fence the project
route. Pane uniqueness is **scope-local** by contract, and the global scan is the
defect.

Read-only verification of the fix hypothesis: with the same replayed index, the
master's pane has 2 claimants globally but exactly 1 within its own scope, so a
scope-local query returns `Some(master)` and the fence passes, while the
cross-scope `codex-%2` stays untouched.

## 2. Goal

- G-A: one explicit, offline, archived, audited reset / cleanup /
  re-initialization operation that clears accumulated control-plane burden.
- G-B: close the normal-path logic gaps, so this class of deadlock stops
  happening and the operator always gets a named, actionable conflict.

## 3. Non-goals

- No sub-agent spawn extension.
- No business payload or message-semantics change.
- No manual editing of route files or token copying.
- No global pane retirement and no implicit retirement of a live conflict. The
  recovery DAG (`docs/design/collab-recovery-dag-audit-20260930.md:9`) keeps
  "explicit user override is the only path that may retire or supersede a live
  conflict".
- No symmetric liveness resolution on the fence. The same-scope stale case is
  demonstrated in data but not observed as a failure; it gets the explicit L1
  remedy instead of new fence logic.
- No control-plane storage relocation in this delivery (D4 below).
- No change to the unmerged `codex/collab-master-liveness-fence` branch.

## 4. Current reset boundary

`collab reset --discard-legacy --approval <text>` (`collab/src/reset.rs:426-438`)
retires the legacy project-local control plane
(`LEGACY_CONTROL_ROOTS = [".agent-collab", ".agent-collab-v2"]`) and rewrites the
host route table to drop that project's records
(`retire_host_routes`, `reset.rs:319-424`; `prune_stale_host_routes`,
`reset.rs:348-367`, called at `reset.rs:586-587`). It requires the daemon to be
down, archives with byte-equality verification, is transactional, and rebuilds an
empty baseline. It does **not** touch the daemon control-plane journal, so it
cannot clear duplicate pane claimants.

Accumulated burden at the host root: 340 identities, 60 archives, 207 route
records, 52 host project directories, 102 live routes, 5 panes with two
claimants, 0 retirements ever.

## 5. Design

### D1 Pane uniqueness is scope-local

Invariant: within one route scope, at most one route claims a given tmux pane.
Two projects may share a pane, and neither may block or retire the other.

- The pane-claimant query takes a route scope and returns the claimant only when
  the scope-local claimant count is exactly one.
- Every scope-specific caller passes its own scope: the fence
  (`runtime_manager_setup.rs:260`), the reconciler (`:287`), register recovery
  (`part_06.rs:1144`, scope already available at `:1129`), staged pane recovery
  (`part_04.rs:137`), committed-register retry (`part_04.rs:219`), and CLI rebind
  (`part_10.rs:789`).
- The host-wide lookup keeps its `Option` contract for callers that genuinely ask
  a host-wide question, and is expressed through the same claimant scan. It is
  not a second mechanism.
- The reducer is unchanged. Cross-scope claimants are never retired by another
  scope's publication.

This alone clears both failure modes: the fence resolves the master, and
`same_pane_tmux_recovery` becomes true so the master can re-register.

### D2 Ambiguity is diagnosable

- One claimant scan owns the question: `pane_route_claimants(scope?, endpoint)`.
  The scope-local lookup, the host-wide lookup, and the pane-only branch of
  `lookup_tmux_route` (`global_state_impl.rs:525-533`, currently a duplicate of
  the same scan) are all expressed through it.
- When the scope-local claimant count is greater than one, the fence error names
  the pane and each claimant (agent id, binding id, generation) and names the
  remedy (`collab reset --routes ... --keep <binding_id>`). Today it reports a
  generation mismatch that hides the real cause.

### D3 Reconcile is reachable, non-aborting, and durable

- A startup `ensure_runtime` failure is logged (host log plus event) instead of
  being discarded by `let _ =` (`runtime_manager_setup.rs:188`).
- `reconcile_same_pane_master_routes` and `reconcile_started_thread_routes` no
  longer abort the whole startup round on the first binding that cannot be
  resolved. They collect the unresolved bindings and report them as one named
  remainder. Daemon start no longer depends on one stale binding.
- The reconciler publishes only the highest-generation binding for a
  `(route_scope, pane)` group, so a retired lower-generation claimant is not
  resurrected at the next start. This is what makes D5 L1 durable; without it the
  owner journal would republish the claimant that L1 just retired.
- The request path stays read-only. No reconcile, commit, or probe is added to
  the fence. Revision 1 proposed that; review P1-5 rejected it as write-on-read
  with an undefined bound.

### D4 Control-plane storage isolation (designed, separate delivery)

Measured: `run_with_host_paths` (`collab/src/server/mod_parts/part_12.rs:848-890`)
replays and appends the host reducer state at `scope.server_dir()`, which is
`<project>/.agent-collab/server/journal.jsonl` for the project the daemon was
started from. `HostPaths::server_dir()` is the state root, and
`<state_root>/journal.jsonl` holds only legacy host events (22 route sets, last
written 2026-09-21). The host control plane therefore shares a journal with one
arbitrary project, and `collab reset --project <that project>` would retire the
host index with it.

Why it is not in this delivery: the root cause has no causal link to the storage
location, and a move loses state that the reconcilers do not rebuild. Review
P1-3 identified the concrete losses: `restore_resident_route_record`
(`runtime_manager_setup.rs:405-445`) returns early when no registration exists,
and master grants and command receipts are replayed only from the journal, not
rebuilt from project bindings. The state-root journal is also not empty, so an
"empty means migrate" trigger cannot move the live index. The relocation needs
its own merge design, its own acceptance case, and its own review.

### D5 Reset operation, three levels

All three levels run offline, need an explicit approval, archive before they
remove, and record one receipt. One entry, three scopes.

| Level | Command | Retires | Keeps |
| --- | --- | --- | --- |
| L1 routes | `collab reset --routes --approval <t> [--storage-root <path>] [--keep <binding_id>]` | stale same-scope pane claimants and their host routes; stale host route records whose project is gone (reusing the existing `prune_stale_host_routes` owner) | all business state (tasks, messages, journals, identities) |
| L2 project | `collab reset --project <path> --approval <t>` | that project's `.agent-collab`, including its project-local business history and its project-local `runs/`, plus its host route records and its identities | other projects and all host state |
| L3 host | `collab reset --host --approval <t> [--storage-root <path>]` | host route table, host control-plane journal, identities, host project directories; with `--storage-root`, also that project's control plane | `~/.collab/runs/` (host run notes and evidence) unless `--include-runs` is given |

`runs` means two different things and the two levels treat them differently on
purpose: L2 removes the project's own `.agent-collab/runs`, and L3 keeps the
host-level `~/.collab/runs` unless `--include-runs` is given. L3's default keeps
the evidence that the user asked to preserve.

L1 rule, deterministic and offline:

- replay the host control-plane journal and group live routes by
  `(route_scope, pane)`;
- a group with one claimant needs nothing;
- a group with several claimants keeps the highest `endpoint_generation` and
  appends `Event::GlobalCurrentThreadRouteRetired { binding }`
  (`collab/src/server/state.rs:760`, reducer `state_impl.rs:761-767`) for each
  other claimant. The reducer removes a route only when the stored binding equals
  the event binding, so L1 replays first and appends the exact stored binding;
- a group whose top generation is tied is a real ambiguity (this is the current
  `$16:%16`). L1 changes nothing, lists the candidates with agent id, binding id,
  and generation, and asks for `--keep <binding_id>`;
- a cross-scope claimant is never a target: it is not a conflict;
- durability: because of D3, the owner journal does not republish a
  lower-generation claimant of the same scope and pane;
- L1 never removes business state and adds no daemon request. The appended event
  is applied when the daemon next replays the journal.

`--storage-root` exists only because of D4: today the live host index sits in the
storage root's project journal. It defaults to the state root, and the receipt
prints the journal path that was used.

`--discard-legacy` keeps its current meaning and is the L2 authorization flag;
when no level flag is given, `collab reset` behaves as today (L2 on the current
project), so existing callers and tests are unchanged.

Shared contract for all three levels:

- non-empty `--approval`; the text is recorded verbatim in the audit record;
- the daemon must be down, proven by the existing reset lock, the legacy writer
  fence, and the liveness check (`reset.rs:444-454`);
- archive first, verify byte equality, then remove; failure leaves the source
  untouched;
- one audit record per run in `<state_root>/reset.jsonl`;
- idempotent: a repeated reset of an already clean plane changes nothing.

## 6. DAG

Two SESE graphs, one per delivery unit. Both pass
`dagpipe graph validate`.

### 6.1 Reset operation

`docs/dagpipe/collab-control-plane-reset.graph.json` (7 nodes, 6 edges)

```text
reset_request
  -> authorize_reset                  (level flag + non-empty approval)
  -> prove_exclusivity                (daemon down, reset lock, writer fence)
  -> inventory_control_plane          (exact retire set, incl. pane claimants)
  -> archive_inventory                (byte-equality verified archive)
  -> retire_selected_state            (transactional retirement)
  -> rebuild_baseline                 (empty, registration-ready baseline)
  -> record_reset_receipt             (reset.jsonl record + stdout contract)
```

The three levels share this pipeline; the level selects the inventory and the
retire set, not the topology. The graph models the success path. The failure
terminals are declared in the graph description and each leaves the source
untouched: `RESET_AUTHORIZATION_REQUIRED`, `RESET_DAEMON_LIVE`,
`RESET_AMBIGUOUS_PANE_CLAIMANTS` (with the candidate list), and
`RESET_ARCHIVE_MISMATCH`.

### 6.2 Pane-route reconciliation

`docs/dagpipe/collab-pane-route-reconcile.graph.json` (5 nodes, 4 edges)

```text
route_index_disagreement
  -> classify_pane_claimants          (scope-local: none / exactly one / several)
  -> resolve_owner_route              (highest-generation scope-local owner)
  -> publish_owner_route              (host index lagged: publish that owner)
  -> verify_scope_pane_uniqueness     (postcondition: one claimant in scope)
  -> emit_reconcile_receipt           (converged, or the named scope-local conflict)
```

Single sink: `reconcile_receipt`, which carries either convergence or the named
remainder. A cross-scope claimant is not a conflict and never appears as one.

## 7. Acceptance

Black-box acceptance (real entry, external observable result):

| Id | Case | Expected |
| --- | --- | --- |
| A1 | Real routecodex master, candidate installed, daemon restarted | `collab context` in routecodex returns an identity with `registered: true`; `collab master status` reports the master; and a recorded window of >= 10 non-`Register` operations from routecodex adds **zero** `RECOVERY_RECONCILE_REQUIRED` lines to `<state_root>/log.txt` (before/after line counts recorded) |
| A2 | Cross-scope pane sharing (`$2:%2`, `$138:%138`) | neither peer is fenced or retired; `collab context` succeeds in both scopes; the host index still shows both claimants |
| A3 | `collab reset --routes --approval <t>` on a state copy with a same-scope duplicate | after the daemon replays, only the highest-generation claimant remains; business state and identities are unchanged; a second run is a no-op; a generation-tied group is refused with its candidates listed |
| A4 | `collab reset --project` and `collab reset --host` | the archive holds the retired bytes, `reset.jsonl` holds the receipt with the approval text, the baseline is rebuilt, and `~/.collab/runs/` survives unless `--include-runs` was given |

White-box regression gate (not a substitute for A1-A4): the full collab test
suite stays green, including
`superseded_same_pane_master_does_not_fence_project_route`,
`same_pane_master_still_fences_when_host_route_is_missing`, and the existing
reset tests.

## 8. Risks and rollback

- D1 changes a shared query. Every non-test call site is listed in D1 and each
  one already filters by scope afterwards, so the scope argument does not change
  its intent.
- D3 stops aborting a startup round on one bad binding. The fence still fails
  closed per request, so an unresolved binding still blocks its own project.
- L1 retires only a strictly lower generation within one scope, and only with an
  explicit approval. Every level archives before it removes.
- Rollback is the reverse commit. No project journal is rewritten; L1 only
  appends.

## 9. File scope

Changed in this delivery:

- `collab/src/server/global_state_impl.rs` (claimant query, scope-local lookup)
- `collab/src/server/mod_parts/runtime_manager_setup.rs` (fence, reconciler)
- `collab/src/server/mod_parts/part_06.rs` (register recovery scope)
- `collab/src/server/mod_parts/part_04.rs` (staged recovery, retry admission)
- `collab/src/server/mod_parts/part_10.rs` (CLI rebind)
- `collab/src/reset.rs`, `collab/src/main.rs` (reset levels)
- tests under `collab/src/server/*_tests*`
- `docs/design/`, `docs/dagpipe/`

`codex/collab-master-liveness-fence` (2 commits, unmerged) changes `part_06.rs`,
`part_07.rs`, `part_08.rs`, `part_09.rs`, `state.rs`. The only shared file is
`part_06.rs`; the merge must re-check the `same_pane_tmux_recovery` predicate
there.
