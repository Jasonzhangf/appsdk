# Collab State Paths and Components

## Global host runtime truth

The host-wide daemon state root is resolved by the installed binary:

```text
$COLLAB_STATE_DIR for isolated tests, else $HOME/.collab
  server.sock
  daemon.lock
  server.pid
  events.jsonl
  log.txt
  routes.jsonl
```

Usage:

- `server.sock` is the live daemon socket. It is created by `collab up` and
  removed by `collab down`. Never write to it by hand.
- `daemon.lock` prevents a second daemon. Do not delete or edit it.
- `server.pid` is the current daemon PID. Verify it with `collab status --all`
  after a controlled `collab down` / `collab up`.
- `events.jsonl` is the append-only durable daemon event stream. Do not edit or
  delete it.
- `log.txt` is diagnostic output. Read it for exact errors; never use it as a
  source of truth for task or identity state.
- `routes.jsonl` is the append-only host-wide admission/storage index for app
  scopes and project roots. It is not a route selector. Never hand-edit it.
  Stale or missing-root routes are retired only through the Collab
  migration/reset owner.

For an explicitly authorized legacy reset, use:

```sh
collab down
collab reset --discard-legacy --approval "<explicit user authorization>"
collab up
collab init
collab status --all
```

Do not `cp`, `grep`, `mv`, truncate, or edit `routes.jsonl`; that bypasses the
owner and destroys route provenance. If existing `.agent-collab/` state must be
preserved, use migration instead of reset.

An ordinary version upgrade is not a reset. Install the current reviewed
binary and refresh the embedded Skill; leave `~/.collab/` and the running
daemon untouched. If the running daemon must load the new binary, use a
separately authorized maintenance window and follow the controlled lifecycle
in [migration-daemon.md](migration-daemon.md).

## Project-local registration input

Each registered project root has its own `.agent-collab/`:

```text
<project>/.agent-collab/
  server/
  runs/
  mailbox/
  messages/
```

`.agent-collab/` is the project registration input and the project runtime
reducer's durable store. The global `~/.collab/` state owns the host socket,
route table, and global identity/liveness records; the project reducer owns
its project-scoped journal, mailbox projection, tasks, claims, and bindings.
No command may reconstruct the current peer from a legacy
`runs/<worker-id>/identity.json`. AppSDK reset must never delete
`.agent-collab/`; use `collab migrate` or the explicit reset lifecycle rather
than deleting files by hand.

For a new project, initialize from the current global version and do not copy
or replay old project-local control files. An old local directory is ignored
unless the operator explicitly chooses the owner's migration or reset route.

## Identity and role

- The current client is Codex only. Identity is bound to the Codex sessionID,
  the internal App Server native thread, and the canonical project cwd.
- A Git worktree does not inherit `.agent-collab/` and is not an identity
  context. Resolve `collab context`, `collab master status`, registration, and
  recovery from the canonical project main tree. The daemon requires
  sessionID, threadID, and canonical cwd to match the same current binding;
  historical routes are not candidates and `routes.jsonl` must not be read to
  guess one. Never register the worktree as a second peer or create a second
  route.
- Default role is `peer`; master is explicit and user-approved.
- `collab context` is the single information endpoint for the current peer,
  binding, role, transport, liveness, tasks, and peers.
- `collab master status` is the authoritative live-master query.
- `collab who` and `collab status --all` are peer diagnostics, not setup steps
  and not a substitute for `collab master status`; `who` has no top-level
  `master` field.
- A failed `collab context`, including `token mismatch`, is a registration
  problem, not evidence that no live master exists. Query `collab master status`
  independently. Only a returned `master: null` with no `recorded_unusable`
  entry permits the explicit user-approved promotion path.

## Transport selection

- Workers never choose transport themselves. The server validates the candidate
  and selects the channel.
- App Server is the supported transport. A selected transport must include its
  server self-check and a live native thread.
- Do not inspect terminal environment paths or infer identity from a pane.

## Registration verification

Run `collab context`. If it says unregistered, run the idempotent
`appsdk init .` (or `collab init` for a standalone project), then run
`collab context` again. Registration must run from the canonical project main
tree, not a `playground/` worktree. `collab context` itself is read-only and
must be run from that same canonical root: it resolves the route from global
state and never creates a route or identity. A worktree cwd fails closed with
`ROUTE_RESOLVE_INVALID`; do not retry it as a second registration.

`collab init` success is not delivery proof. Verify the live binding, selected
transport, endpoint liveness, presence, and role through `collab context`.
Never edit `routes.jsonl`, `server.pid`, journal, mailbox, or identity files to
make a registration appear healthy.

If `collab context` fails with `token mismatch`, preserve the exact error and
stop registration repair. Do not copy a global token into project state, edit
an identity file, run a reset, or promote a peer. Check `collab master status`
separately, report the exact context error to the live master, and use the
migration/reset owner only when that owner explicitly decides the project-local
control plane is unrecoverable.

## App Server endpoint and thread recovery

A registered route is live only when the selected App Server endpoint owns the
same native thread and the thread is loaded. `persisted but not loaded`,
`notLoaded`, or an active-writer conflict means the durable registration and
the current TUI runtime are on different endpoints; it is not a reason to
re-register, edit `routes.jsonl`, copy a token, or start another daemon.

Diagnose read-only from the canonical project main checkout:

```sh
collab context
collab route resolve --session-id <session-id> --native-thread-id <thread-id>
collab worker status <peer-id>
```

Record the selected endpoint, native thread ID, binding generation,
`endpoint_live`, `identity_valid`, `presence`, `agent.thread_state`, and the
exact error. The recovery owner must then prove the endpoint owner:

1. The endpoint in the selected transport must be the App Server that owns the
   current Codex TUI/thread. A managed control socket is valid only when the
   current thread is loaded there.
2. If the current TUI owns the thread but is not connected to the selected
   managed endpoint, the route endpoint must be corrected through the
   supported peer rebind/recovery path. Do not point the route at a test or
   disposable socket.
3. If the selected App Server reports the thread as not loaded after recovery
   on the current connection, do not call `turn/start` or `turn/steer`. The
   current contract fails closed when native recovery cannot load the thread.
4. If the endpoint reports an active writer conflict, identify the exact
   lock/owner and resolve the runtime ownership. Do not kill a process by name,
   remove a lock by hand, or start a second daemon.
5. After the endpoint/thread owner is corrected, run `collab worker recover`
   once, then `collab context`. Success requires `endpoint_live=true`,
   `identity_valid=true`, `presence=present`, and a loaded thread. A command
   acceptance without those fields is not recovery.

The canonical project main checkout owns identity lookup, registration, and
recovery. Never create a worktree-local peer or endpoint to make the route
appear live.
