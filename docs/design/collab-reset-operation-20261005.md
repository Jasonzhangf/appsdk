# Collab control-plane reset operation

Delivery 2 of the collab control-plane work. Delivery 1 is the normal-path fix
(scope-local pane uniqueness, named ambiguity, ensure-runtime logging) and is
designed in `collab-control-plane-reset-20261005.md`, revision 3. This document
covers the explicit reset/cleanup/initialization operation that clears
accumulated control-plane burden.

Revision 2, 2026-10-05. Status: implemented, pending verification. Revision 2
folds in the independent design review: the compaction requirement
(`snapshot_events`), the replay-helper and request-path consumers, the decision
to drop the offline liveness gate, the register-path reactivation rule,
repeatable `--keep`, the `archives/` archive root, the L3 retire set, and the
correction of two factual claims about existing code.

## 1. Proven problem

The control plane accumulates history that no normal operation can clear. The
live host index measured on 2026-10-05:

| Burden | Measured |
| --- | --- |
| Panes with more than one route claimant | 5 (`$2:%2` cross scope, `$6:%6`, `$8:%8`, `$16:%16` same scope, `$138:%138`) |
| `routes.jsonl` records | 207, for 207 distinct storage roots, including roots that no longer exist |
| Identities | 340 |
| Archives | 60 |
| Host project directories | 52 |
| `reset.jsonl` records | 19 |
| Journal events | 7869 in the appsdk host journal, 111 route sets, **0 retirements** |

Zero retirements in this journal is the core fact. The event type already
exists: `Event::GlobalCurrentThreadRouteRetired` (`state.rs:760`) is emitted by
the register path when `retire_cross_project_anchor` retires a foreign anchor
(`runtime_manager_setup.rs:626`), and `retire_current_thread_route`
(`global_state_impl.rs:840-868`) removes the route from the index. This journal
holds zero such events, and neither mechanism is reachable from an operator
command, so a claim disappears only when a *newer* route replaces it.

The existing `collab reset` (`reset.rs:426`) is level 2 only: it retires the
current project's legacy project-local control plane and rebuilds an empty
baseline, and it needs `--discard-legacy` plus `--approval`. There is no way to

- retire one named route claimant on a pane that has several, or
- rebuild the host control plane (identities, archives, routes, journals).

The user asked for exactly these, in this order: first a complete
reset/cleanup/initialization operation that clears the accumulated burden, then
the normal-path gaps.

## 2. Goal

One explicit operation with three levels. Every level is auditable,
transactional, and needs explicit authorization.

- **L1 `collab reset --routes`** retires named route claimants in one scope and
  keeps exactly one. It targets the duplicate-pane burden.
- **L2 `collab reset --project`** rebuilds the current project's runtime
  baseline. This is the existing behavior, with its boundary stated precisely.
- **L3 `collab reset --host`** rebuilds the host control plane: identities,
  archives, route records, and the host journals. It keeps `~/.collab/runs/`
  unless `--include-runs` is given.

Deliverable: the three levels, their tests, black-box acceptance B1-B6, and the
DAG in section 6.

## 3. Non-goals

- No change to the normal-path route rules. Delivery 1 owns those.
- No automatic or background cleanup. Every level is operator-invoked and needs
  `--approval`.
- No deletion of `~/.collab/runs/` by default. Run notes are the durable record
  of a run and survive a host reset unless the operator asks for their removal.
- No migration. `collab migrate` keeps history; reset discards it. The two do
  not merge.
- No change to `~/.collab/service.json`. It belongs to an external supervisor
  and is not a dependable collab contract.
- No MCP tool for the destructive levels. The CLI is the only entry, so an
  agent cannot reach a reset through a tool call.

## 4. Why an index-level retirement is not durable

This is the problem review rounds 3 and 4 raised, and it decides the design.

`retire_current_thread_route` removes the route from the in-memory index and
writes nothing (`global_state_impl.rs:862-867`). Four separate paths then put
the claim back:

1. **Replay.** The journal still holds the `GlobalCurrentThreadRouteSet` event
   for that address (`state_impl.rs:757-767`), so the next start replays the
   route into the index. This is also why the *existing* retirement event is not
   enough on its own: `Event::GlobalCurrentThreadRouteRetired` removes the route
   during replay but leaves no durable statement that the address must stay
   retired, and it carries no operator authorization.
2. **`reconcile_started_thread_routes`** republishes any binding of any project
   runtime that carries a session and a native thread whose host route is
   missing (`runtime_manager_setup.rs:400-421`). The project journal owns that
   binding, so the host index is rebuilt from the project side.
3. **`reconcile_same_pane_master_routes`** does the same for a pending same-pane
   master (`runtime_manager_setup.rs:343-349`).
4. **`restore_unique_current_thread_routes_from_bindings`** rebuilds the live
   index from `projects[*].runtime_bindings` when the journal has no
   `GlobalCurrentThreadRouteSet` at all (`state_impl.rs:3-61`, called at
   `part_12.rs:492-495`). It calls `set_current_thread_route` for every durable
   binding whose address is missing, so it re-creates the claim and clears any
   retirement record as a side effect.

Path 2 also explains the failure mode the user reported: a stale project binding
keeps a route alive on a pane that a different project's master now owns.

The existing tombstone cannot express an operator retirement.
`RuntimeBindingTombstone` carries `rebound_to: RuntimeBinding` and
`new()` requires the old and the new binding to share project, app scope, agent,
and binding id (`global_state_models.rs:405-432`). That is a *rebind* record: it
says "this address moved to that binding". An operator retirement has no
successor binding, so it has no valid `rebound_to`.

The tombstones do have production consumers —
`lookup_current_thread_route_tombstone` and `lookup_tmux_route_tombstone` are
read by the address resolver (`part_04.rs:252-338`, reached from `:114` and
`:122`) to report `SESSION_THREAD_BINDING_STALE`. A retired address must take the
opposite branch there: it must not resolve. `resolve_route_by_address` already
returns `ROUTE_RESOLVE_NOT_FOUND` (`part_04.rs:335-338`) for an address that is
not in the index, and that is the correct outcome for a claim an operator
removed, so this delivery adds no branch to the resolver. What the resolver must
not do is find a route the operator retired, and the four consumers above are
what guarantee that.

The full list of consumers that must consult the retired record is therefore:
the two reconcilers, the replay helper, and the request-path fence
(`same_pane_master_route_ready`, `runtime_manager_setup.rs:251-319`, reached from
`part_04.rs:1193` and `:1236`). A record that only the reconcilers consult would
leave the request path fencing the project forever.

A successful registration is deliberately *not* on that list. Registering again
is live activity, and `set_current_thread_route` clears the retirement for
exactly that address, so the peer comes back only by proving it is running.
Neither reconciler can produce that proof, which is the whole difference.

So L1 needs its own durable record and its own consumers.

## 5. Design

### 5.1 CLI surface

```
collab reset --routes  --storage-root <path> --keep <binding_id> --approval <text> [--discard-legacy]
collab reset --project [<path>]                                  --approval <text> [--discard-legacy]
collab reset --host    --storage-root <path>                     --approval <text> [--discard-legacy] [--include-runs]
```

- The three level selectors are mutually exclusive and exactly one is required.
  A run with none or with two fails with `RESET_LEVEL_REQUIRED` before it reads
  or writes any control file.
- `--storage-root` is required for L1 and L3, and is rejected for L2. For L1 it
  names the root whose project journal holds the live pane routes:
  `<storage-root>/.agent-collab/server/journal.jsonl`, read by the same
  `replay_host_index` the daemon uses (`part_12.rs:359`). L1 canonicalizes the
  root, requires that journal to exist, and then requires the journal to hold at
  least one route or project for that canonical root whenever it holds any record
  at all. A root that owns nothing is `RESET_STORAGE_ROOT_INVALID`. `routes.jsonl`
  cannot stand in for it: all 207 records carry
  `canonical_root == storage_root`, so the file does not identify which root the
  running daemon uses. A default would silently no-op on the wrong index, which
  is why the flag is required rather than optional.
- `--keep <binding_id>` is required for L1 whenever any pane in the named index
  has more than one claimant in one route scope. It is **repeatable**, and it
  names the survivor per ambiguous pane: each ambiguous pane must contain
  exactly one of the ids given. A pane that contains none of them, or more than
  one, is a conflict. A run with any conflict changes nothing, lists every
  conflicting pane with its claimants, and exits with `RESET_KEEP_REQUIRED`.
  L1 never guesses by generation: `endpoint_generation` is a per-binding counter
  that starts at 1 (`part_04.rs:159,211`), so it is not comparable across
  bindings.
- A flag that the selected level does not use is an error
  (`RESET_LEVEL_FLAG_MISMATCH`), not a silent no-op. `--keep` is L1 only,
  `--include-runs` is L3 only, and `--storage-root` is rejected for L2. Silently
  ignoring a flag on a destructive operation is how an operator comes to believe
  they asked for something they did not.
- `--discard-legacy` stays required, as today, so the destructive path cannot be
  reached by a bare `collab reset`.
- L2 takes no path argument. It acts on the current project root, resolved the
  way the existing command resolves it, so B4 runs `collab reset --project` from
  the storage root rather than passing it a path.

### 5.2 L1: retire named route claimants

L1 is offline. It refuses to run while a daemon answers on the socket
(`RESET_DAEMON_LIVE`, the existing gate at `reset.rs:448-453`), so it never
mutates a live index.

Steps:

1. **Authorize.** Require `--approval` with non-blank text and
   `--discard-legacy`, as today (`reset.rs:427-438`).
2. **Resolve the index.** Read `<storage-root>/.agent-collab/server/journal.jsonl`
   and build the view with the same function the daemon uses:
   `replay_from_journal` plus `restore_unique_current_thread_routes_from_bindings`
   and `index_legacy_thread_routes_from_bindings` (`part_12.rs:492-501`). Using a
   private reader is not allowed: the appended retirement event must satisfy the
   reducer's equality precondition, and only the daemon's own replay defines the
   binding the reducer will compare against.
3. **Select.** Group the live routes by `(route_scope, pane)` through the same
   scope-local claimant scan delivery 1 introduced. `RouteScope` is the app scope
   plus the project scope, so two projects that share one pane are never one
   group, and a scope-local decision stays scope-local. "Pane" is the full tmux
   pane identity — socket path, server pid, tmux session id, pane id, and pane
   pid — matching `same_pane_route` and `tmux_route_address`. Keying on
   `session:pane` alone would merge two different panes that reuse a session and
   pane number on separate tmux servers, or on a recreated pane, and the run
   would then force the operator to name a survivor over a set that is not a
   duplicate. A group of one is not a target. For each group of two or more, the
   single claimant named by `--keep` is kept and every other claimant is a
   retirement target. The error and the audit record print all five fields, so
   two panes retired in one run stay distinguishable.
4. **No liveness gate.** L1 does not probe tmux and does not ask whether the
   target's worker is registered. Both claimants of one pane share the same pane
   and the same `pane_pid`; they differ only in their Codex address, and deciding
   which one is still real needs a live tmux probe that an offline command must
   not perform. A gate of the form "refuse while the target is registered" would
   refuse exactly the stale claims the operator needs to retire, because a stale
   claim stays registered until something retires it. `--keep` is the
   authorization and the liveness decision: the operator names the survivor, and
   everything else on that pane is residue by that decision.
5. **No archive, and why.** L1 removes nothing. It appends one event per target
   to a journal that keeps every earlier event, so the pre-image is the file
   itself and there are no retired bytes to preserve. The audit trail is the
   receipt in `<state_root>/reset.jsonl` plus the events in the journal, and the
   decision is reversible: a later route set at the same address reactivates it.
   Levels 2 and 3 do remove bytes, and both stage them into
   `<state_root>/archives/<label>-<run_id>/` first (`archive_retired`,
   `reset.rs:611`), with the file count, byte count, and tree digest.
6. **Commit.** Append one `GlobalRouteClaimRetired` event per target (section
   5.3) to the index journal, `sync_data`, then `sync_all` the directory. One
   append per target, in a stable order, so a partial failure is visible as a
   prefix. The append first repairs a missing trailing newline, because the
   append path assumes a whole-line record (`part_02.rs:820-825`) while the
   compaction rewrite does not always end with one (`part_02.rs:1060-1101`).
   A failure restores the journal from the whole-file pre-image snapshot
   `snapshot_file` took before the append, rather than truncating to a recorded
   offset, because the append may also have repaired the final newline.
7. **Verify.** Replay the journal and assert that each retired address is absent
   from the live index and carries a retired record, and that every claimant named
   by `--keep` is still live. This runs inside the same transaction as the append
   and before the receipt, so a journal that does not carry the retirement cannot
   leave a receipt that claims it did. A mismatch restores the pre-image and fails
   with `RESET_VERIFY_FAILED`.
8. **Receipt.** Append one record to `<state_root>/reset.jsonl` with the level,
   the approval text, the run id, the archive path, the retired claims, the kept
   binding ids, the digest, and the timestamp (`append_reset_record`,
   `reset.rs:197`).

L1 is idempotent: a second run finds no group with more than one claimant and
changes nothing.

### 5.3 The durable retirement record

Add one typed record and one event. The record answers "this claim was retired
by an operator and must not be republished"; it is not a rebind, so it does not
reuse `RuntimeBindingTombstone`.

```
RetiredRouteClaim {
    binding: RuntimeBinding,
    approval: String,
    reason: String,
    at_ms: i64,
}

Event::GlobalRouteClaimRetired { record: RetiredRouteClaim }
```

Reducer rule (`state_impl.rs`):

1. Remove the live route for `record.binding`, exactly as
   `retire_current_thread_route` already removes it.
2. Insert `record` into `GlobalState.retired_route_claims`, keyed by
   `retired_route_claim_key`, which is the same `current_route_address_key`
   (`session id`, `native thread id`, tmux endpoint) the route index uses. A
   repeat of the same record is a no-op, so replaying a re-appended event is safe.
3. Reject a record whose binding does not validate. Because step 1 removes the
   route in the same event, a live route and a retirement for the same address
   cannot both come out of the reducer. `GlobalState::validate` still rejects that
   pair, fail-closed, so a hand-edited or partially written journal stops startup
   instead of silently resurrecting the claim.

Reactivation. `set_current_thread_route` clears the retirement for the address it
is installing. Registering again is live activity, and the register path is the
only producer that can prove it, so a peer that really is running comes back by
registering. The reconcilers cannot prove it, which is the whole difference.

Compaction. `rewrite_journal_locked` builds the compacted journal from
`State::snapshot_events` alone, and `purge_expired_storage` runs at startup
before the reconcilers (`part_12.rs:892-893`). The retirement must therefore be
re-emitted from `snapshot_events`, after the route sets and after the tombstone
events, so the reducer removes the claim rather than re-installing it. A record
that is only in the in-memory map is lost on the first compaction.

Consumers. Four places consult the map, and each records the skip:

- `reconcile_started_thread_routes` (`runtime_manager_setup.rs:400`): before the
  publish, skip a retired address and log
  `RECOVERY_RECONCILE_SKIPPED_RETIRED: binding <id> agent <agent> was retired by
  an operator; not republished`.
- `reconcile_same_pane_master_routes` (`runtime_manager_setup.rs:326`): the same
  skip before its publish.
- `restore_unique_current_thread_routes_from_bindings` (`state_impl.rs:3-61`):
  skip a retired address, otherwise the replay helper re-creates the claim and
  clears the record in the same step.
- `same_pane_master_route_ready` (`runtime_manager_setup.rs:251-319`): treat a
  retired pending claimant as resolved rather than as a same-pane conflict.
  Without this the request path fences every non-Register request in the project
  forever, because the claimant it is waiting on can never be published.

The request-path fence also names its remedy now. The single-different-claimant
branch and the `host and project pane routes disagree` error both report the
pane, the scope, the owner, the binding, and the generation, and both point at
`collab reset --routes --keep <binding_id>`. An operator who is told only that
two routes disagree cannot tell which one to keep.

This is the only place the design changes startup behavior, and it is fail-closed
in the right direction: the skip is gated on an explicit, operator-authorized,
durable record, not on an error. Nothing else changes.

### 5.4 L2: rebuild the project baseline

L2 keeps today's behavior and states its boundary:

- It retires the current project's legacy project-local control plane and
  rebuilds an empty baseline, archiving the retired bytes.
- It clears business history by default. Tasks, messages, and identities of that
  project are archived, not merged.
- It refuses when the project holds the host index, because retiring that tree
  would delete the index the daemon replays. The refusal is
  `RESET_PROJECT_HOLDS_HOST_INDEX` and points to L3. This is B4. The owner is
  read from `service_scope_root` in `<state_root>/service.json`. A descriptor
  that exists but cannot be read, parsed, or resolved is an error
  (`RESET_INDEX_ROOT_UNRESOLVED`), not a silent "no owner": the level must not
  fail open on a corrupt descriptor and retire the daemon's own index root. Only
  an absent descriptor means no root holds the live index.
- Existing safety checks stay: `reject_unsafe_control_roots`
  (`reset.rs:154`), `reject_symlinked_guidance` (`:225`), the reset lock and the
  legacy writer fence (`:444-447`).

### 5.5 L3: rebuild the host control plane

L3 is the "complete reset" the user asked for. It requires `--storage-root`,
because the live index it must clear is spread across two roots: the host
control plane under `<state_root>` and the resident project's runtime journal
under `<storage_root>/.agent-collab/server/`.

Retired, staged, and archived:

| Target | Path |
| --- | --- |
| Host route records | `<state_root>/routes.jsonl` |
| Host journal | `<state_root>/journal.jsonl` |
| Host events | `<state_root>/events.jsonl` |
| Host log | `<state_root>/log.txt` |
| Host identities and project directories | `<state_root>/identities/`, `<state_root>/projects/` |
| Resident index journal | `<storage_root>/.agent-collab/server/journal.jsonl` |
| Resident index events and log | `<storage_root>/.agent-collab/server/{events.jsonl,log.txt}` |
| Run notes | `<state_root>/runs/`, only with `--include-runs` |

Kept, and why:

- `<state_root>/reset.jsonl` and `<state_root>/archives/`. These are the audit
  trail of every reset, including this one. Deleting them would make the
  operation unauditable, and the archive of this run is written under
  `archives/`, so removing the directory would remove the evidence for the run
  that is still in progress.
- `<state_root>/service.json`, `build-version*`, `daemon.lock`, `server.pid`, and
  `server.sock`. `service.json` describes an external supervisor's desired state;
  the rest are the daemon's own liveness files, and `collab down` owns them.
  L3 never deletes a live socket.
- `<storage_root>/.agent-collab/{mailbox,messages,handoff,merge-queue,runs,mailboxes}`.
  That is project business payload, not control plane, and clearing it is L2's
  job, where the operator names the project. L3 clears the control plane of the
  root it was given, not the business history of every project under it.
- `<state_root>/runs/` unless `--include-runs` is given. Run notes are the
  durable per-run record and must survive a control-plane reset by default.

No baseline is rebuilt. L3 leaves both roots without a live index, and the next
`collab up` recreates the daemon's own state from empty, exactly as a first
start does. L2 is the level that rebuilds a project baseline.

`--storage-root` must be an existing directory that contains
`.agent-collab/server/journal.jsonl`, or L3 fails with
`RESET_STORAGE_ROOT_INVALID` before staging anything. This keeps a typo from
archiving an unrelated directory. The same ownership check as L1 applies: a root
whose journal holds records for a different root is refused.

### 5.6 Transaction, audit, and failure terminals

Every level uses the same transaction shape, which already exists in
`reset.rs`:

```
authorize -> level select -> lock -> commit -> verify -> receipt
                                     |
                            rollback on any failure

L2 and L3 insert `stage` before `commit` and `discard_staged_roots` after the
receipt, because they remove bytes. L1 removes nothing, so it has no stage step
and no discard step.
```

- Staging copies bytes and records counts and a digest (`stage_retired_roots`),
  and only L2 and L3 stage.
- A failure before the receipt calls `rollback_retired_roots`, which restores the
  staged bytes. For L1 the journal and `reset.jsonl` are each restored from the
  pre-image snapshot `snapshot_file` took before the transaction.
- `discard_staged_roots` only runs after the receipt is durable, so the archive is
  never removed before the audit record exists.
- `reset.jsonl` is append-only and holds the approval text, so the audit trail
  names who authorized the run.

Failure terminals, each a DAG node with acceptance evidence:

| Terminal | Condition |
| --- | --- |
| `RESET_LEVEL_REQUIRED` | No level, or two levels |
| `RESET_LEVEL_FLAG_MISMATCH` | A flag the selected level does not use |
| `RESET_AUTHORIZATION_REQUIRED` | Missing `--discard-legacy` or blank `--approval` |
| `RESET_STORAGE_ROOT_REQUIRED` | L1 or L3 without `--storage-root` |
| `RESET_STORAGE_ROOT_INVALID` | `--storage-root` is not a collab storage root, or owns no record in the journal it names |
| `RESET_DAEMON_LIVE` | A daemon answers on the socket |
| `RESET_KEEP_REQUIRED` | An ambiguous pane has no single claimant named by `--keep` |
| `RESET_PROJECT_HOLDS_HOST_INDEX` | L2 on the root that holds the index |
| `RESET_INDEX_ROOT_UNRESOLVED` | L2 cannot read or resolve the daemon's `service.json` descriptor |
| `RESET_VERIFY_FAILED` | Post-commit replay does not match the intent |

There is no liveness terminal. L1 deliberately does not test whether a target is
still live, for the reason in step 4 above, so `--keep` is the only gate between
the operator and a retirement.

## 6. DAG

Single source `collab reset`, single sink the receipt plus its verification.

The graph is the one already registered as `appsdk-collab-control-plane-reset`,
extended to `0.2.1`. It gains one node, `verify_retirement`, between the
retirement commit and the baseline rebuild, so a postcondition that does not
hold cannot reach the receipt.

Failure terminals are **bound to the node that produces them**, not modelled as
extra sink nodes. `dagpipe graph validate` enforces that an audited SESE graph
declares exactly one output arc, so a terminal cannot also be a node; the first
draft of this graph was rejected with
`an audited SESE Graph must declare exactly one output ARC`. Each terminal
therefore names its producing node and its acceptance case:

| Producing node | Terminal | Acceptance |
| --- | --- | --- |
| `authorize_reset` | `RESET_LEVEL_REQUIRED` | B0 |
| `authorize_reset` | `RESET_LEVEL_FLAG_MISMATCH` | B0 |
| `authorize_reset` | `RESET_AUTHORIZATION_REQUIRED` | existing reset tests |
| `authorize_reset` | `RESET_STORAGE_ROOT_REQUIRED` | B1 |
| `authorize_reset` | `RESET_STORAGE_ROOT_INVALID` | B7 |
| `prove_exclusivity` | `RESET_DAEMON_LIVE` | existing reset tests |
| `inventory_control_plane` | `RESET_KEEP_REQUIRED` | B3 |
| `inventory_control_plane` | `RESET_PROJECT_HOLDS_HOST_INDEX` | B4 |
| `verify_retirement` | `RESET_VERIFY_FAILED` | B9 |

`verify_retirement` is the L1 postcondition, and it is the only level that has
one. `run_project` and `run_host` do not call it: neither retires a live route
set that a later start could republish, so neither has a retirement to verify.
`run_project` carries `is_current_empty_baseline` instead, and that is an
idempotence **pre-check** before archiving (`reset.rs:751-757`), not a
post-retirement assertion. The graph's single linear chain therefore describes
the L1 path; treat the chain, not the per-level call graph.

Success path for L1, in order: `authorize_reset` -> `prove_exclusivity` ->
`inventory_control_plane` -> `archive_inventory` -> `retire_selected_state` ->
`verify_retirement` -> `rebuild_baseline` -> `record_reset_receipt`.

The graph is registered in `docs/dagpipe/manifest.json` and validated with
`dagpipe graph validate` (8 nodes, 7 edges, 8 waves, exit 0). The design-graph
operator allow-list in `rust/src/dagpipe.rs` must name every operator the graph
uses, in node order: `verify_retirement` sits between `retire_selected_state`
and `rebuild_baseline` in both. A node's `operator_version` is the operator's
version, not the graph's, and the registry serves every design operator at
version `"1"`, so raising the graph's own `version` does not change it.

Every node maps to code that exists, so the graph is not ahead of the
implementation:

| Node | Code |
| --- | --- |
| `authorize_reset` | `ResetLevel::select`, `ResetRequest::validate_level_flags`, and the `--approval`/`--discard-legacy` check in `reset::run` |
| `prove_exclusivity` | `acquire_reset_lock` plus the `client::alive` gate in each level |
| `inventory_control_plane` | `resolve_index_root` and `pane_claimant_groups` (L1), `LEGACY_CONTROL_ROOTS` and `reject_unsafe_control_roots` (L2), `host_control_plane_entries` (L3) |
| `archive_inventory` | `archive_retired` over `tree_digest`, `copy_tree`, and `stage_retired_roots` |
| `retire_selected_state` | `append_retirement_events` (L1), the legacy-root retire loop and `retire_host_routes` (L2), the removal of `host_control_plane_entries` (L3) |
| `verify_retirement` | `verify_retirement` in `run_routes`: replay the index with `replay_host_index` and assert the postcondition (L1 only; no other level calls it) |
| `rebuild_baseline` | the L2 baseline rebuild in `run_project`; a no-op for L1 and L3 |
| `record_reset_receipt` | `append_reset_record`, then `discard_staged_roots` |

`rollback_retired_roots` is the failure edge out of `retire_selected_state`, not
a node: it restores the pre-image of the node that failed and returns the error.

## 7. Acceptance

Black box, from the real CLI entry.

| Id | Case | Expected |
| --- | --- | --- |
| B0 | `collab reset` with no level, and with two levels; then `collab reset --project --storage-root <root>` | `RESET_LEVEL_REQUIRED` and `RESET_LEVEL_FLAG_MISMATCH`; nothing changes |
| B1 | `collab reset --routes` without `--storage-root`, while the live index is not at the state root | `RESET_STORAGE_ROOT_REQUIRED`; nothing changes |
| B2 | `collab reset --routes --storage-root <root> --keep <binding_id> --approval <t>` on a state copy with a same-scope duplicate | after the daemon replays, the named claimant is retired, the kept one is the only claimant of that pane, business state and identities are unchanged, and a second run is a no-op |
| B3 | The same command with an ambiguous pane that `--keep` does not resolve | nothing changes; every conflicting pane and its claimants are listed; the exit code is non-zero |
| B4 | `collab reset --project` run from inside the storage root, and with a `service.json` that is unreadable, malformed, or has no resolvable `service_scope_root` | the first is refused with `RESET_PROJECT_HOLDS_HOST_INDEX` pointing to L3; the rest are refused with `RESET_INDEX_ROOT_UNRESOLVED` naming the descriptor. A project whose `service.json` is absent is retired normally |
| B5 | `collab reset --project`, then `collab reset --host --storage-root <root>`, then `collab up` | L2 archives the project tree and rebuilds its baseline; L3 archives both control-plane roots and leaves no index; `reset.jsonl` holds both receipts with their approval text; `~/.collab/runs/` survives unless `--include-runs` was given |
| B6 | Durability: after B2, start the daemon twice | the retired claim does not come back; the index fingerprint is identical across the two starts. The reconciler skip that prevents the republish is asserted by unit test, because an isolated fixture cannot register a peer on a live pane, so the reconciler has no binding to walk and the skip is not observable in the host log |
| B7 | `collab reset --host --storage-root <a directory that is not a collab root>` | `RESET_STORAGE_ROOT_INVALID`; nothing is staged |
| B8 | `collab reset --routes` on a copy with two ambiguous panes in one scope and two `--keep` flags | both stale claims are retired in one run and both survivors remain |
| B9 | `collab reset --routes` on a copy whose post-commit replay cannot satisfy the postcondition | `RESET_VERIFY_FAILED`; the journal is restored from its pre-image snapshot and no receipt is written |
| B10 | A pane-only candidate registers in project A while project B holds a pane-only anchor on the same pane | the candidate is not rejected as ambiguous; project B is untouched unless `retire_cross_project_anchor` is set, and then the existing retirement path runs unchanged |
| B11 | Startup with a pending same-pane master whose pane has one in-scope claimant that is not itself, plus a cross-scope claimant | the daemon starts or fails with a named conflict that carries the `collab reset --routes --keep` remedy; it never fails with an unnamed disagreement |
| B12 | A peer registers at an address that was retired, without a daemon restart | the registration succeeds, the address is live again, and its retired record is gone |

B6 is the acceptance that makes L1 real. Without it, L1 is cosmetic.

## 7.1 Residuals this delivery absorbs

The independent architecture review of delivery 1 returned PASS and left three
open items. They live in the same files and the same invariant family as this
delivery, so this delivery closes them instead of leaving a known gap.

- **P2-1, the register path still asks a host-wide pane question.** FIXED.
  `validate_current_thread_candidate` (`runtime_manager_setup.rs:580-712`)
  collects anchor matches from every runtime's project bindings and rejects with
  `RUNTIME_BINDING_REJECTED: tmux identity anchor matches multiple persisted
  peers` when more than one matches. Its pane arm applies when the candidate
  carries no Codex ids, so a pane-only candidate counted a claimant in another
  project on the same pane. Two projects may share a pane, so this breaks the
  delivery 1 invariant from a different call site. The register path now counts
  ambiguity only among same-scope matches and prefers the in-scope match as the
  anchor, so a single cross-scope match stays on the existing
  `retire_cross_project_anchor` path. When nothing matches in scope, the previous
  fail-closed rule for several foreign matches is unchanged. Acceptance: B10.
- **P3-1, a startup input class whose direction changed.** FIXED.
  `reconcile_same_pane_master_routes` publishes a pending same-pane master when
  the in-scope lookup finds no claimant. When a scope has exactly one claimant X
  that is not the pending binding, and another project also claims the pane, the
  host-wide query used to return `None` and publish, while the in-scope query now
  returns X and fails. The new direction is fail-closed and it refuses to create
  a second in-scope claimant, which is the invariant delivery 1 exists to
  protect, but it could stop the daemon from starting with no way forward. The
  durable retirement rule in section 5.3 is the fix: a pending master whose pane
  is already owned in scope now fails with a named conflict that reports the
  pane, the scope, both owners, both generations, and the
  `collab reset --routes --keep <binding_id>` remedy, and a pending master whose
  own address was retired is treated as resolved instead of as a conflict.
  Acceptance: B11.
- **P3-4, the reconcile graph is ahead of the code.** FIXED.
  `docs/dagpipe/collab-pane-route-reconcile.graph.json` named
  `resolve_owner_route` and `emit_reconcile_receipt` as nodes, but the reconciler
  returns `Result` and there is no `ReconcileReceipt` type, so the graph promised
  a persisted artifact that nothing produces. The graph is now `0.3.0` and names
  the code that exists: `classify_pane_claimants`, `resolve_scope_pane_owner`,
  `publish_owner_route`, `verify_scope_pane_uniqueness`, and
  `return_named_outcome`, whose output is the reconciler's own outcome instead of
  a receipt. Acceptance: `dagpipe graph validate` reports
  `valid DAG: appsdk-collab-pane-route-reconcile@0.3.0 (5 nodes, 4 edges,
  5 waves)`.

White-box regression gate, not a substitute: the full collab suite stays green,
including the existing reset tests and the two delivery-1 pane tests.

## 8. Risks and rollback

- **A live peer is retired by mistake.** `--keep` is the only gate, by design
  (section 5.2 step 4). The operator names the survivor, and everything else on
  that pane is residue by that decision. The record makes the decision auditable,
  and a peer that really is running comes back by registering at the same
  address, which clears the record. The mitigation is therefore reversible rather
  than preventive, and it is bounded: the run retires at most the claimants of
  panes that already have more than one claimant in one scope.
- **The retirement record blocks a legitimate future rebind.** The reducer
  clears a retirement record for an address when a new route is set for that
  address. A worker that comes back, with or without a new binding id, is not
  blocked.
- **A stale record outlives its claim.** The record is cleared on reactivation
  and is re-emitted from `snapshot_events`, so it cannot be lost by compaction
  and cannot silently accumulate: one address has at most one record.
- **L3 removes evidence.** Everything retired is staged and archived before it
  is removed, and the receipt is written before the staged copy is discarded.
  `~/.collab/runs/` survives by default.
- **A wrong `--storage-root` archives the wrong tree.** L3 validates that the
  root holds a collab journal before staging.
- **Rollback.** Each level is one transaction with a recorded pre-image. A
  failure restores it. The merge-time rollback for a regression is a `git
  revert` of the merge commit, as the delivery rules require.

## 9. File scope

- `collab/src/reset.rs` — the three levels, the retired-claim inventory, staging,
  verification, and receipts.
- `collab/src/main.rs` — the `Reset` command flags and dispatch.
- `collab/src/server/global_state_models.rs` — `RetiredRouteClaim` and the
  `GlobalState.retired_route_claims` map.
- `collab/src/server/global_state_impl.rs` — the map's lifecycle: record,
  lookup, clear-on-reactivation, and validation.
- `collab/src/server/state.rs` — `Event::GlobalRouteClaimRetired`.
- `collab/src/server/state_impl.rs` — the reducer branch, the `snapshot_events`
  emission, and the replay helper's skip.
- `collab/src/server/mod_parts/part_12.rs` — `replay_host_index`, the replay
  entry L1 uses to read the index.
- `collab/src/server/mod_parts/part_06.rs` — the comment recording why a
  successful registration deliberately re-activates a retired address.
- `collab/src/server/mod_parts/runtime_manager_setup.rs` — the two reconciler
  skips, the replay-path skip, the request-path skip, the scope-local
  `validate_current_thread_candidate` (P2-1), and the named conflict errors with
  their `collab reset --routes --keep` remedy.
- `collab/src/server/mod.rs` — the `RetiredRouteClaim` re-export.
- `collab/src/reset_tests.rs` — level gating, multi-pane L1, and the refused-run
  cases.
- `collab/src/server/host_route_registry_tests/part_02_tail.rs` — the reducer,
  compaction, replay-helper, and request-path regression tests.
- `docs/dagpipe/collab-control-plane-reset.graph.json` — the extended reset
  graph, `0.2.0`.
- `docs/dagpipe/collab-pane-route-reconcile.graph.json` — the reconcile graph
  rewritten to match the reconciler, `0.3.0` (P3-4).
- `docs/dagpipe/manifest.json` — graph registration.

- `collab/skills/collab/SKILL.md` — the operator description of the three levels.
- `collab/skills/collab/references/state-paths.md` — the per-level sequences.

Delivery 1 owns `global_state_impl.rs` and `runtime_manager_setup.rs` too. This
delivery rebased onto the merged delivery 1 (`72810b0`) before its own review,
and the two deliveries are not reviewed as one candidate.
