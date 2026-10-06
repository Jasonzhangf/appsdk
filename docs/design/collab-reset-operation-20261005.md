# Collab control-plane reset operation

Delivery 2 of the collab control-plane work. Delivery 1 is the normal-path fix
(scope-local pane uniqueness, named ambiguity, ensure-runtime logging) and is
designed in `collab-control-plane-reset-20261005.md`, revision 3. This document
covers the explicit reset/cleanup/initialization operation that clears
accumulated control-plane burden.

Revision 3, 2026-10-06. Status: implemented. Revision 3 removes the former
routes level: one tmux pane now owns exactly one route binding host-wide, so no
operator command resolves an ambiguous pane. `collab reset` has exactly two
levels, `--project` and `--host`. See
`collab-pane-route-ownership-20261006.md` section 6.

## 1. Proven problem

The control plane accumulates history that no normal operation can clear. The
live host index measured on 2026-10-05:

| Burden | Measured |
| --- | --- |
| `routes.jsonl` records | 207, for 207 distinct storage roots, including roots that no longer exist |
| Identities | 340 |
| Archives | 60 |
| Host project directories | 52 |
| `reset.jsonl` records | 19 |
| Journal events | 7869 in the appsdk host journal, 111 route sets |

Before this delivery, `collab reset` was the project level only: it retires the
current project's legacy project-local control plane and rebuilds an empty
baseline, and it needs `--discard-legacy` plus `--approval`. There was no way to
rebuild the host control plane (identities, archives, routes, journals).

The user asked for a complete reset/cleanup/initialization operation that clears
the accumulated burden, then the normal-path gaps.

## 2. Goal

One explicit operation with two levels. Every level is auditable,
transactional, and needs explicit authorization.

- **L2 `collab reset --project`** rebuilds the current project's runtime
  baseline. This is the existing behavior, with its boundary stated precisely.
- **L3 `collab reset --host`** rebuilds the host control plane: identities,
  archives, route records, and the host journals. It keeps `~/.collab/runs/`
  unless `--include-runs` is given.

Deliverable: the two levels, their tests, black-box acceptance B0, B4, B5, B7,
and B10, and the DAG in section 5.

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

## 4. Design

### 4.1 CLI surface

```
collab reset --project                       --approval <text> [--discard-legacy]
collab reset --host    --storage-root <path> --approval <text> [--discard-legacy] [--include-runs]
```

- The two level selectors are mutually exclusive and exactly one is required.
  A run with none or with two fails with `RESET_LEVEL_REQUIRED` before it reads
  or writes any control file.
- `--storage-root` is required for L3 and rejected for L2. It names the root
  whose project journal holds the live pane routes:
  `<storage-root>/.agent-collab/server/journal.jsonl`, read by the same
  `replay_host_index` the daemon uses (`part_12.rs:359`). L3 canonicalizes the
  root, requires that journal to exist, and then requires the journal to hold at
  least one route or project for that canonical root whenever it holds any record
  at all. A root that owns nothing is `RESET_STORAGE_ROOT_INVALID`. `routes.jsonl`
  cannot stand in for it: all 207 records carry
  `canonical_root == storage_root`, so the file does not identify which root the
  running daemon uses. A default would silently no-op on the wrong index, which
  is why the flag is required rather than optional.
- A flag that the selected level does not use is an error
  (`RESET_LEVEL_FLAG_MISMATCH`), not a silent no-op. `--include-runs` is L3 only,
  and `--storage-root` is rejected for L2. Silently ignoring a flag on a
  destructive operation is how an operator comes to believe they asked for
  something they did not.
- `--discard-legacy` stays required, as today, so the destructive path cannot be
  reached by a bare `collab reset`.
- L2 takes no path argument. It acts on the current project root, resolved the
  way the existing command resolves it, so B4 runs `collab reset --project` from
  the storage root rather than passing it a path.

### 4.2 L2: rebuild the project baseline

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
  (`reset.rs:290`), `reject_symlinked_guidance` (`:361`), the reset lock and the
  legacy writer fence (`:732-735`).

### 4.3 L3: rebuild the host control plane

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
archiving an unrelated directory. A root whose journal holds records for a
different root is refused.

### 4.4 Transaction, audit, and failure terminals

Both levels use the same transaction shape, which already exists in
`reset.rs`:

```
authorize -> level select -> lock -> commit -> receipt
                                     |
                            rollback on any failure

L2 and L3 insert `stage` before `commit` and `discard_staged_roots` after the
receipt, because they remove bytes.
```

- Staging copies bytes and records counts and a digest (`stage_retired_roots`),
  and only L2 and L3 stage.
- A failure before the receipt calls `rollback_retired_roots`, which restores the
  staged bytes. `reset.jsonl` is restored from the pre-image snapshot
  `snapshot_file` took before the transaction.
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
| `RESET_STORAGE_ROOT_REQUIRED` | L3 without `--storage-root` |
| `RESET_STORAGE_ROOT_INVALID` | `--storage-root` is not a collab storage root, or owns no record in the journal it names |
| `RESET_DAEMON_LIVE` | A daemon answers on the socket |
| `RESET_PROJECT_HOLDS_HOST_INDEX` | L2 on the root that holds the index |
| `RESET_INDEX_ROOT_UNRESOLVED` | L2 cannot read or resolve the daemon's `service.json` descriptor |

## 5. DAG

Single source `collab reset`, single sink the reset receipt.

The graph is registered as `appsdk-collab-control-plane-reset`, version `0.3.0`.
It has seven nodes in one linear chain, and it carries no separate postcondition
node: neither `--project` nor `--host` retires a live route set that a later
start could republish, so neither has a retirement to verify.

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
| `authorize_reset` | `RESET_STORAGE_ROOT_REQUIRED` | existing reset tests |
| `authorize_reset` | `RESET_STORAGE_ROOT_INVALID` | B7 |
| `prove_exclusivity` | `RESET_DAEMON_LIVE` | existing reset tests |
| `inventory_control_plane` | `RESET_PROJECT_HOLDS_HOST_INDEX` | B4 |

Success path, in order: `authorize_reset` -> `prove_exclusivity` ->
`inventory_control_plane` -> `archive_inventory` -> `retire_selected_state` ->
`rebuild_baseline` -> `record_reset_receipt`.

The graph is registered in `docs/dagpipe/manifest.json` and validated with
`dagpipe graph validate` (7 nodes, 6 edges, 7 waves, exit 0). The design-graph
operator allow-list in `rust/src/dagpipe.rs` must name every operator the graph
uses, in node order. A node's `operator_version` is the operator's version, not
the graph's, and the registry serves every design operator at version `"1"`, so
raising the graph's own `version` does not change it.

Every node maps to code that exists, so the graph is not ahead of the
implementation:

| Node | Code |
| --- | --- |
| `authorize_reset` | `ResetLevel::select`, `ResetRequest::validate_level_flags`, and the `--approval`/`--discard-legacy` check in `reset::run` |
| `prove_exclusivity` | `acquire_reset_lock` plus the `client::alive` gate in each level |
| `inventory_control_plane` | `resolve_index_root` and `host_control_plane_entries` (L3), `LEGACY_CONTROL_ROOTS` and `reject_unsafe_control_roots` (L2) |
| `archive_inventory` | `archive_retired` over `tree_digest`, `copy_tree`, and `stage_retired_roots` |
| `retire_selected_state` | the legacy-root retire loop and `retire_host_routes` (L2), the removal of `host_control_plane_entries` (L3) |
| `rebuild_baseline` | the L2 baseline rebuild in `run_project`; a no-op for L3 |
| `record_reset_receipt` | `append_reset_record`, then `discard_staged_roots` |

`run_project` carries the idempotence pre-check `is_current_empty_baseline`
before archiving (`reset.rs:779`).

`rollback_retired_roots` is the failure edge out of `retire_selected_state`, not
a node: it restores the pre-image of the node that failed and returns the error.

## 6. Acceptance

Black box, from the real CLI entry.

| Id | Case | Expected |
| --- | --- | --- |
| B0 | `collab reset` with no level, and with two levels; then `collab reset --project --storage-root <root>` | `RESET_LEVEL_REQUIRED` and `RESET_LEVEL_FLAG_MISMATCH`; nothing changes |
| B4 | `collab reset --project` run from inside the storage root, and with a `service.json` that is unreadable, malformed, or has no resolvable `service_scope_root` | the first is refused with `RESET_PROJECT_HOLDS_HOST_INDEX` pointing to L3; the rest are refused with `RESET_INDEX_ROOT_UNRESOLVED` naming the descriptor. A project whose `service.json` is absent is retired normally |
| B5 | `collab reset --project`, then `collab reset --host --storage-root <root>`, then `collab up` | L2 archives the project tree and rebuilds its baseline; L3 archives both control-plane roots and leaves no index; `reset.jsonl` holds both receipts with their approval text; `~/.collab/runs/` survives unless `--include-runs` was given |
| B7 | `collab reset --host --storage-root <a directory that is not a collab root>` | `RESET_STORAGE_ROOT_INVALID`; nothing is staged |
| B10 | A pane-only candidate registers in project A while project B holds a pane-only anchor on the same pane | the candidate is not rejected as ambiguous; project B is untouched unless `retire_cross_project_anchor` is set, and then the existing retirement path runs unchanged |

### 6.1 Residuals this delivery absorbs

The independent architecture review of delivery 1 returned PASS and left open
items. Two of them live in the same files and the same invariant family as this
delivery, so this delivery closes those two instead of leaving a known gap.

- **P2-1, the register path still asked a host-wide pane question.** FIXED.
  `validate_current_thread_candidate` (`runtime_manager_setup.rs:498-615`)
  collects anchor matches from every runtime's project bindings and rejects with
  `RUNTIME_BINDING_REJECTED: tmux identity anchor matches multiple persisted
  peers` when more than one matches. Its pane arm applies when the candidate
  carries no Codex ids, so a pane-only candidate counted a claimant in another
  project on the same pane. Delivery 1 let two projects share a pane, so that
  broke the delivery 1 invariant from a different call site; the register path
  counts ambiguity only among same-scope matches and prefers the in-scope match
  as the anchor, so a single cross-scope match stays on the existing
  `retire_cross_project_anchor` path. When nothing matches in scope, the previous
  fail-closed rule for several foreign matches is unchanged. The later
  pane-ownership reversal in `collab-pane-route-ownership-20261006.md` does not
  change this rule: it answers an identity question, and the binding ledger can
  still hold more than one record for a pane. Acceptance: B10.
- **P3-4, the reconcile graph is ahead of the code.** FIXED.
  `docs/dagpipe/collab-pane-route-reconcile.graph.json` named
  `resolve_owner_route` and `emit_reconcile_receipt` as nodes, but the reconciler
  returns `Result` and there is no `ReconcileReceipt` type, so the graph promised
  a persisted artifact that nothing produces. The graph now names the code that
  exists: `classify_pane_claimants`, `publish_owner_route`, `return_named_outcome`
  (whose output is the reconciler's own outcome instead of a receipt), and, at
  its current revision `0.4.0`, `resolve_pane_owner` and `verify_pane_uniqueness`
  (renamed from the scope-local names when pane ownership became global).
  Acceptance: `dagpipe graph validate` reports
  `valid DAG: appsdk-collab-pane-route-reconcile@0.4.0 (5 nodes, 4 edges,
  5 waves)`.

White-box regression gate, not a substitute: the full collab suite stays green,
including the existing reset tests and the two delivery-1 pane tests.

## 7. Risks and rollback

- **L3 removes evidence.** Everything retired is staged and archived before it
  is removed, and the receipt is written before the staged copy is discarded.
  `~/.collab/runs/` survives by default.
- **A wrong `--storage-root` archives the wrong tree.** L3 validates that the
  root holds a collab journal before staging.
- **Rollback.** Each level is one transaction with a recorded pre-image. A
  failure restores it. The merge-time rollback for a regression is a `git
  revert` of the merge commit, as the delivery rules require.

## 8. File scope

- `collab/src/reset.rs` — the two levels, staging, and receipts.
- `collab/src/main.rs` — the `Reset` command flags and dispatch.
- `collab/src/server/mod_parts/part_12.rs` — `replay_host_index`, the replay
  entry L3 uses to read the index.
- `collab/src/server/mod_parts/runtime_manager_setup.rs` — the in-scope
  `validate_current_thread_candidate` anchor rule (P2-1).
- `collab/src/reset_tests.rs` — level gating and the refused-run cases.
- `docs/dagpipe/collab-control-plane-reset.graph.json` — the reset graph, `0.3.0`.
- `docs/dagpipe/collab-pane-route-reconcile.graph.json` — the reconcile graph
  rewritten to match the reconciler, `0.4.0` (P3-4).
- `docs/dagpipe/manifest.json` — graph registration.

- `collab/skills/collab/SKILL.md` — the operator description of the two levels.
- `collab/skills/collab/references/state-paths.md` — the per-level sequences.

Delivery 1 owns `global_state_impl.rs` and `runtime_manager_setup.rs` too. This
delivery rebased onto the merged delivery 1 (`72810b0`) before its own review,
and the two deliveries are not reviewed as one candidate.
