# collab workflow

This project uses the local `collab` daemon for multi-agent coordination.
The source and build truth lives in the AppSDK repository's `collab/` directory;
the installed independent binaries are `~/.cargo/bin/collab` and
`~/.cargo/bin/collab-mcp`. From an AppSDK checkout, use
`scripts/install-global-collab.sh`; do not build a second copy from an external
Collab checkout.

The daemon is detached. Normal commands may start it when no explicit `DOWN`
marker exists. `collab init` creates the global `~/.collab` persistence directory,
so old projects need no manual repair. Use
`collab down` only for an explicit stop; use `collab up` to clear that stop and
start it again. Never start a second daemon.
Existing projects migrate through `collab migrate inspect`, `plan`, `apply`,
controlled daemon upgrade/restart, identity rebind, and `verify`;
deleting `~/.collab`, editing JSON state, clearing mailboxes, copying
tokens, mixed runtime writes, and guessing thread identity are deprecated.

## Runtime boundary

- Every peer registration is admitted by the server after transport
  self-check. App Server is the only supported transport; a registration that
  cannot verify a live native route fails explicitly.
- Registration creates or refreshes one reusable default `direct-message`
  lease with a bounded 600-second TTL; inspect its current status and expiry
  with `collab notify status`.
- App Server delivery means the native queue accepted a bounded preview; it is
  not execution, read, or reply.
- Server state, journal, and mailbox are durable truth; a failed wake cannot
  roll back state or fabricate success.
- The runtime is part of the worker identity boundary, not a task preference.

## Roles

- Every registered identity is an equal `peer`; there is no inferred master
  from first registration. A host agent root is not Collab master.
- `collab init` and peer registration never create a master. A master exists
  only when a registered peer has a live transport and was assigned by
  user-approved self-promotion or live-master delegation. A recorded identity
  with a dead transport is not a live master.
- If a live master exists, other peers cannot promote; only that master may
  `collab master delegate <peer>`. If no live master exists, a peer may
  `collab master promote --approval "<user text>"` itself after explicit user
  approval. Master authority is arbitration only; it does not take another
  peer's task. Independent peers may temporarily decline a master
  collaboration invite to protect their own task; managed subagents must obey
  the master.
- Each peer self-registers one task and owns its full worktree, test,
  integration, main verification, push, cleanup, and resource lifecycle.
- Task owner, resource holder, integration lease, and daemon operator are
  scoped capabilities, never durable identity roles.
- Peers send no normal progress reports. P2P communication is limited to
  durable resource occupancy and release coordination.

## Task lifecycle

```
working -> verifying -> reviewed -> delivered
        -> owner sync/verify/integrate -> merged -> cleanup_pending
        -> cleanup_verified -> closed
        -> rework -> working
blocked -> bounded waiting -> resource release/timeout -> owner recheck
```

Task records use a fixed shape:
`id / owner / feature_id / worktree_path / branch / base_commit / priority /
 status`. Normal statuses are `working`, `blocked`, `waiting`, `verifying`,
 `reviewed`, `delivered`, `rework`, `merged`, `closed`, and `cancelled`.

## Common commands

```sh
collab up                         # clear explicit down and start daemon
collab down                       # explicit stop; disables auto-restart
collab who                        # registered peers + local state projection
collab task status [task-id]      # durable task registry
collab notify methods             # discover opt-in notification methods
collab notify subscribe --event direct-message --ttl-seconds 600
collab notify status
collab context                    # read-only authoritative state snapshot
collab master status              # live master, or recorded-but-dead identity
collab master promote --approval "<user text>"
collab master delegate <peer>     # live master only
collab task register <id> --feature <feature-id> --worktree <path> \
  --branch <branch> --base-commit <sha> --priority p2
collab task wait <id> --for <blocking-task>
collab task deliver <id> --evidence "commit=<sha>; gates=pass" --worktree <path>
collab task block <id> --next "blocked: <evidence and next condition>"
collab task update <id> --status merged
collab task close <id>            # owner; verifies merged/clean, releases claim
```

Peers never share worktrees. Each task owner starts from latest main in one
declared clean `./playground/` worktree, implements and tests, commits the exact
change set, syncs latest main again, verifies the candidate, acquires a short
integration lease, merges the exact commit to main, verifies and pushes main,
then closes the task to remove only its clean merged worktree/branch and persist
a cleanup receipt. A bound worktree is a mandatory cleanup obligation;
`delivered`/`merged` are not cleanup completion, and a task with a pending or
unproven cleanup cannot become closed or pass audit. Delivery is an owner-local
durable milestone and sends no peer notification. `/goal`
delegation and interactive task recognition are intentionally deferred.

## Message handling

On a notification, use its id and abbreviated subject to weigh urgency against
the current task. Query durable state before acting when the notice is relevant.
`collab sendmessage` requires `--subject` and accepts only explicit coordination
or asynchronous-result notices. Never type peer messages directly; the daemon
sends through the selected transport. After the receiving Agent registers a
finite subscription, the daemon may send one id,
abbreviated subject, safe one-line original body preview, and final submit key
as one submit through the server-selected transport. App Server uses
`thread/queue/add`. The direct-message lease is reusable until expiry; resource,
deadline, and async-result subscriptions remain one-shot.

`collab inbox` and `collab msg <id>` query the durable local mailbox after a
transport is unavailable; mailbox state remains authoritative.

## Notifications and waits

There is no periodic continuation. Agent-owned subscriptions are exact-event,
exact-subject, and finite. Direct-message delivery is serialized and reusable
until expiry; other subscriptions are one-shot. No registration, absent,
unknown, working, expired, cancelled, consumed, or exhausted message produces
transport input. Every wait stores waiter, blocking task owner, reason, deadline,
resume events, and P2P escalation. Timeout changes state without unsolicited
messages; resource release notifies only an exact active subscriber.
