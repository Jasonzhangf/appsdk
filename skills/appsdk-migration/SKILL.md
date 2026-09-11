---
name: appsdk-migration
description: >
  Migrate an existing AppSDK and Collab installation to one reviewed version,
  preserving business and protected state while safely resetting an explicitly
  authorized legacy control plane. Use for version upgrades, daemon
  migrations, identity rebinding, and authorized legacy resets; do not use for
  ordinary feature development or unapproved deletion.
---

# AppSDK migration

Use this Skill when an existing AppSDK or its Collab runtime must move to a
reviewed version, when a daemon or peer identity must be rebound, or when the
operator has explicitly authorized discarding a named legacy control plane.
The migration owner runs the whole operation and records its evidence. A
worker may inspect or prepare a candidate, but may not independently delete
state, restart the daemon, rebind another peer, or resume admissions.

This Skill owns the migration state machine:

```text
prepare -> inspect -> classify -> snapshot -> freeze
        -> install/restart -> identity-rebind -> verify -> resume
```

The installed `collab` Skill owns transport, daemon, route, and identity
semantics. The AppSDK project-governance Skill owns AppSDK quality gates and
the canonical `appsdk reset-governance --discard-legacy` operation. Read those
Skills before acting; do not copy their state machines into this one. For the
project-level preserve/reset choices, also read
[`bootstrap-migration.md`](../appsdk-project-governance/references/bootstrap-migration.md).

## Invariants

- There is one global Collab daemon for the host. Resolve its PID and socket
  before changing anything. Never start a second daemon, use a project-local
  replacement, or repair a timeout by switching binaries or sockets.
- A project scope is the exact project root/cwd resolved by the authoritative
  runtime. A tmux pane, process, screenshot, mailbox, or shared directory by
  itself does not prove a registered identity or route.
- A durable peer registration and its live runtime route are the identity
  authority. A transcript/session ID is observation metadata and may change
  after compression, fork, pane replacement, or restart. Rebind the new live
  runtime through the official registration path; never copy tokens or make a
  session ID the durable identity.
- User authorization is required before promoting a peer to `master`. A
  Codex TUI can register and communicate only through a real live route. A
  Desktop runtime must not register a goal subscription; only the authorized
  master/TUI scheduler may do so.
- The communication path and durable facts are different surfaces. A tmux or
  appserver notification is a bounded wake hint; the journal/mailbox is the
  durable record. Do not delete, rewrite, replay, or use a notification as
  proof of consumption.
- Business source, runtime data, human documents, `active/`, and `protected/`
  remain retained by default. Their age or filename is not evidence that they
  are disposable.
- Only the exact legacy control objects named in the approved migration plan
  may be removed, and only through the canonical reset/migration command. Do
  not hand-edit or manually remove journals, mailbox files, identity tokens,
  task records, bindings, claims, or worker state.
- A timeout, missing receipt, `unknown`, partial response, identity mismatch,
  or count mismatch is a failed gate for progression. Preserve the exact
  error, stop dependent writes, and do not claim success, retry in a tight
  loop, or fall back to an older writer.

## Prepare

Before inspecting mutable state, write a migration record with a unique
`run_id` and bind:

- the exact project root/cwd and appserver/runtime scope;
- the migration owner, authorized operator text, and requested action
  (`preserve`, `canonical-migrate`, or `discard-legacy`);
- the source version/binary and the reviewed candidate version, commit, tree,
  artifact, and expected SHA-256;
- the exact worktree and branch used for any code or Skill change;
- the acceptance gates, non-goals, and the path where append-only evidence is
  stored.

For the AppSDK source repository itself, first determine whether the root is a
managed AppSDK project. The presence of SDK source or `.appsdk-control/` alone
does not make it a normal governed application. If there is no project
contract, do not run `appsdk init` or reset commands just to manufacture one;
perform only the declared SDK/runtime migration and record that scope.

All code, Skill, or contract changes are made in a clean non-`main` worktree
created from the latest `origin/main`. Merge and verify the candidate on the
intended mainline before installing it. Do not develop in a dirty root or in
the worktree that owns the live daemon.

## Inspect

Inspection is read-only. Capture the current state before choosing a reset or
migration route. Use the official commands available in the installed
versions, with the usual Collab sequence:

```sh
collab migrate inspect
collab status --all
collab context
```

Also inspect the AppSDK project contract and current lifecycle state through
its read-only commands. Resolve, at minimum:

- one daemon PID, socket, binary path, version, and SHA-256;
- project root/cwd, current branch/HEAD, all worktrees, and any unmerged
  worker branch;
- registered peers, roles, parent/owner relations, live panes, appserver
  routes, tasks, leases, claims, and active goal subscriptions;
- journal, mailbox, notification, task, worker, and identity counts plus their
  last durable IDs;
- `.appsdk/`, `.appsdk-control/`, generated roots, `active/`, `protected/`,
  runtime data, business source, and reports outside declared generated roots;
- current errors, in-flight transactions, freezes, and owner conflicts.

The inspection result must say whether the old daemon, project-local socket,
goal record, or control directory is authoritative, stale, or unknown. Do not
infer this from a PID file, mtime, or filename. If a local daemon appears to
exist, prove its PID/socket ownership and its relation to the global daemon
before taking any lifecycle action.

If inspection returns `PROJECT_SCOPE_UNKNOWN`, an identity error, a command
timeout, or an unavailable status, record that exact result and stop before
classification that could delete state. A live daemon is not enough to pass
inspection.

## Classify

Create a path-level classification in the migration plan. Every item is one
of `retain`, `migrate`, `discard-through-canonical-command`, or `unknown`.

| Class | Default treatment | Examples |
| --- | --- | --- |
| `retain` | Keep in place and include its evidence reference | business source, runtime data, human documents, `active/`, `protected/`, valid external build/release output |
| `migrate` | Let the supported migration preserve/transform it once | supported daemon state, registered identity metadata, current task/lease records |
| `discard-through-canonical-command` | Remove only after authorization and freeze | old `.appsdk/` records/transactions/maps, stale audit or migration reports, `.appsdk-control/`, declared rebuildable generated projections |
| `unknown` | Retain and escalate; do not mutate | ambiguous PID/socket, external report, failed staging tied to an active task, unrecognized state file |

For the idempotent reset route that discards the named legacy control plane,
the exact command is:

```sh
appsdk reset-governance --discard-legacy
```

Run it once, only in the clean non-`main` owner worktree after the named
objects and authorization are recorded. The command is idempotent and owns
removal of its declared legacy control set. Do not replace it with `rm`, a
glob, a directory rename, manual JSON edits, or a second reset attempt.

When the user explicitly chooses to abandon the old governance epoch and start
the existing project from the current SDK baseline, use the fresh-init entry:

```sh
appsdk init <project> --fresh --discard-legacy
```

This is the only init path that discards a legacy control plane. It requires an
existing `.appsdk/project.json`, a clean non-`main`/`master` worktree, and the
explicit `--discard-legacy` confirmation. It removes the same AppSDK-owned
`.appsdk/`, `.appsdk-control/`, and declared generated roots, then rebuilds the
current `.appsdk` state and refreshes SDK-managed record/transition contracts.
It records `mode: "fresh_init"`. Ordinary `appsdk init` remains non-destructive;
the lower-level reset command remains idempotent. A rejected fresh-init must
leave the old state untouched.

The canonical transition contract is `contracts/transitions/zone-transition.manifest.json`.
The historical `contracts/transitions/zone-transition-manifest.json` path remains
supported as a project declaration and must always be refreshed from that same
canonical content; it is not a separate governance history.

The reset does not authorize removal of `active/`, `protected/`, runtime data,
business source, `dist/`, `.deploy/`, `build/`, `tmp/`, or custom reports.
Obsolete Active/Protected or external artifacts need their own exact-path
authorization and cleanup record. Do not copy old PASS, hashes, receipts, or
review results into the new baseline. The reset record proves the reset only.

If old state belongs to a still-valid task, use that task's canonical abort or
retry operation before reset. If its owner cannot be established, classify it
as `unknown` and stop; deletion would hide an ownership or delivery failure.

## Snapshot

Take the immutable snapshot before the freeze or any removal. Store it in the
approved append-only run note or audit location outside the discard set. A
snapshot is provenance, not a replacement control plane and not a new PASS
receipt.

The snapshot records:

- `run_id`, UTC timestamp, operator/owner, project root/cwd, branch/HEAD, and
  worktree path;
- daemon PID/socket/binary/version/SHA-256 and the exact reviewed candidate;
- every classified path with its class, reason, owner, and retention or
  removal authorization;
- peer/role/parent/route identities, task and lease IDs, goal state, and
  journal/mailbox/notification/worker counts;
- the last durable IDs and a hash or size/count manifest for retained
  evidence;
- active errors, in-flight operations, and the command output that established
  each fact.

Do not put credentials, identity tokens, or secret payloads in the snapshot.
Do not use a copied snapshot to satisfy a later live check; after restart,
query the authoritative store again.

## Freeze

Freeze admissions and mutable migration inputs only after the snapshot passes
its completeness check. Use the official migration transaction, which holds
one transaction lease:

```sh
collab migrate plan
collab migrate apply
```

The plan must name the exact source, destination, retained classes, discard
classes, owner, and release condition. Stop new dispatches, goal registration,
and state-changing writes while the freeze is held. Do not freeze by deleting
files or killing processes. A worker notification or a direct message does
not authorize maintenance.

If a freeze lease is already held, ownership is ambiguous, or an operation is
in flight, preserve the existing state and stop. Do not steal the lease,
force-close another owner, or start a second migration transaction.

Timeouts are observation boundaries, not a reason to use a ten-second retry
loop. On a busy system, query the supported migration status once with a
bounded wait, preserve `in_progress` or `unknown` when no receipt exists, and
escalate to the migration owner. Resume only from the same canonical
transaction if the command explicitly supports it.

## Install and restart

Install only the reviewed candidate that is already merged and verified on the
intended mainline. Verify its version and SHA-256 before changing the live
daemon. Installation alone does not replace a running process.

Use the official service lifecycle:

```sh
collab down
collab up
```

If `collab migrate apply` owns the restart, do not run a second manual restart;
follow the command's receipt. Otherwise keep the daemon explicitly down while
the reviewed candidate is installed, then bring up exactly one daemon. Never
use `pkill`, `killall`, broad process matching, a second socket, or an older
binary as an implicit fallback.

After the restart, prove one PID and socket, the expected binary hash, and no
old writer. A restart error or ambiguous process ownership is `unknown`; do
not rebind peers or send recovery messages until the owner resolves it.

## Identity rebind

Rebind only the named live peers after the daemon and socket pass the restart
gate. The current Codex TUI must be a real registered tmux runtime with a
working bidirectional route. Use the official current-peer initialization or
rebind operation once in that pane, then inspect:

```sh
collab context
collab whoami
collab status --all
```

The evidence must bind the durable peer ID to the live runtime/pane, exact
project cwd, appserver route (if present), role, parent, and capabilities. A
screen preview, process name, pane number, or session ID alone is insufficient.

When a pane is replaced or a transcript is forked/compressed, preserve the
durable peer identity only through the supported authenticated rebind. Do not
reuse a stale pane, guess among panes, register a new master, copy identity
tokens, or replay the old mailbox batch. A user-approved master assignment is
the only basis for the `master` role; otherwise the peer remains a peer.

If any identity, scope, parent, or capability differs from the snapshot, stop
before messaging. Record `identity_mismatch` and require an explicit
operator/master repair decision.

## Verify

Verification is ordered and each gate needs its own evidence. Passing one gate
does not imply the next.

1. **Runtime:** one global daemon/PID/socket, expected binary/version/hash,
   no duplicate or old writer, and a successful official status query.
2. **Durability:** journal, mailbox, tasks, workers, leases, and last durable
   IDs are continuous with the snapshot, apart from explicitly recorded
   migration events. Any unexplained count or ID loss fails the gate.
3. **Identity:** the AppSDK directory's Codex TUI has a confirmed durable
   identity, exact cwd/project scope, role, and live bidirectional route.
4. **Communication:** send one unique migration marker to an authorized
   registered peer and require separate evidence for durable journal
   acceptance, notification delivery, peer consumption, and a reply. `sent`,
   `pane evidence`, or `recv` alone is not an end-to-end success receipt.
5. **Governance:** after a reset, initialize and compile only the fresh current
   contract, then run `appsdk verify` through the project governance Skill.
   Verify that no discarded legacy goal/control record is active and that no
   old PASS or receipt was imported.
6. **Scope:** only the approved paths were removed; business, runtime,
   `active/`, `protected/`, and retained evidence remain present and owned.

If the real marker cannot be consumed or the reply is missing, record the
first failed layer and stop. Do not call a mailbox append, tmux wake, or
status response a communication proof.

## Resume

Resume only after every applicable verification gate has a current receipt:

- release the migration freeze through its canonical command;
- re-read authoritative daemon, identity, task, and goal state;
- restore the one authorized master and only its valid routes;
- let the master dispatch new work from the fresh/current plan;
- register a long-horizon goal only from the authorized master/TUI when the
  user requested ongoing wakeups. Desktop must not run `goal subscribe`;
- send one aggregated recovery/completion notice that points to the durable
  run record. Do not replay every old notification.

If any gate is missing, leave the system frozen or in its explicitly reported
partial state, retain the snapshot and exact error, and hand the next action
to the migration owner. “Daemon is up” is not permission to resume work.

## Failure and unknown-error contract

Use this contract at every phase:

| First divergence | Required action | Forbidden action |
| --- | --- | --- |
| inspect/status timeout or unavailable scope | preserve exact output; stop before classification/removal | infer identity, retry in a tight loop, or claim healthy |
| incomplete snapshot or count/hash mismatch | keep current state; repair snapshot inputs or escalate | freeze with incomplete truth or overwrite the snapshot |
| lease/owner conflict | preserve both owners and task IDs; ask the migration owner to resolve | steal, force-close, or invent an owner |
| candidate/version/hash mismatch | stop before install; report expected and observed values | install “close enough” or use the old binary silently |
| restart timeout or ambiguous PID/socket | leave lifecycle state explicit; inspect once through the official command | send/rebind, start a second daemon, or kill by process name |
| identity/scope mismatch | stop all messaging and goal operations; perform authenticated rebind or escalate | copy tokens, guess pane/session, or promote a peer |
| partial reset/migration result | preserve transaction ID and files; use the canonical recovery path | manually finish deletion or run a second reset |
| communication marker missing reply | classify the failing layer and keep the route unverified | treat durable send or notification as peer consumption |

An unknown error is not a transient success. A later attempt is allowed only
when the canonical command reports the previous transaction state and the
migration owner explicitly continues that same transaction. Never hide an
unknown by falling back, changing the project scope, or creating a new daemon.

## Evidence record

Write one append-only JSONL event for each phase transition and each failed
gate. Use this shape; omit secrets and large payloads:

```json
{"schema":"appsdk-migration/v1","run_id":"20260910T000000Z-example","at":"2026-09-10T00:00:00Z","phase":"inspect","operation":"collab migrate inspect","result":"pass","actor":{"peer_id":"peer-id","role":"master","runtime_id":"runtime-id","tmux_pane":"%2","cwd":"/absolute/project"},"source":{"head":"commit","tree":"tree","binary":"/absolute/bin","version":"0.1.6","sha256":"hex"},"evidence":{"paths":["/absolute/run-note.jsonl"],"counts":{"tasks":0,"workers":0,"mailbox":0},"receipts":["receipt-id"],"errors":[]},"retained":["active/","protected/"],"discarded":[],"next":"classify"}
```

Required semantics:

- `result` is exactly `pass`, `fail`, `unknown`, or `skipped`; an absent
  result is incomplete evidence.
- `source` identifies the observed candidate; `actor` identifies the real
  runtime that performed the operation. Do not substitute a transcript ID for
  either.
- `evidence` contains paths, counts, receipts, and exact error codes/text
  references. It must not contain credentials or copied control tokens.
- `retained` and `discarded` list only classified paths; `discarded` requires
  the authorization and canonical-operation receipt elsewhere in the record.
- A new baseline may reference the inventory for history, but it must not
  rebuild control state from this JSONL or inherit old PASS claims.

The migration is complete only when the final `resume` event is `pass`, all
applicable verification receipts are present, the exact removed/retained paths
are reported, and no unresolved `unknown` or owner conflict remains.
