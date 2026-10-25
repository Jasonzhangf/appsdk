# Collab State Paths and Components

## Global host runtime truth

The host-wide daemon state root is resolved by the installed binary:

```text
$COLLAB_STATE_DIR for isolated tests, else $HOME/.collab
  server.sock
  daemon.lock
  server.pid
  events.jsonl
  journal.jsonl
  log.txt
  routes.jsonl
  projects/<encoded-project>/apps/<encoded-app>/.agent-collab/server/
```

Usage:

- `server.sock` belongs to the global user-managed daemon. Agent commands do not
  create or remove it. Never write to it by hand.
- `daemon.lock` prevents a second daemon. Do not delete or edit it.
- `server.pid` is the current daemon PID. Read it through `collab context` or
  `collab status --all`; do not use it to control the user-managed service.
- `events.jsonl` records service lifecycle diagnostics. It does not select an
  identity or reconstruct task state.
- `journal.jsonl` stores context operation receipts. Runtime reducer journals
  live in the central project/app namespace. Do not edit either journal.
- `log.txt` is diagnostic output. Read it for exact errors; never use it as a
  source of truth for task or identity state.
- `routes.jsonl` is the host-wide admission/storage index for app
  scopes and project roots. It is not a route selector. Never hand-edit it.
  The daemon automatically retires missing or deinitialized project routes
  and routes whose loaded reducer binding is absent or superseded.
  Project reset removes target route history through the same writer owner.

The current project master may reset the current project online:

```sh
collab reset --project
```

Run it from the current project. The daemon deletes that project's historical
messages, routes, tasks, assets and load. It preserves other projects and the
global daemon. There is no offline prerequisite or archive prerequisite, and
historical pane owners or open work do not block the reset. Real persistence,
I/O or input-safety errors remain explicit. No legacy discard mode or separate
approval argument exists for this project reset.

Do not `cp`, `grep`, `mv`, truncate, or edit `routes.jsonl`; that bypasses the
owner and destroys route provenance. If existing `.agent-collab/` state must be
preserved, use migration instead of reset.

An ordinary version upgrade is not a reset. Install the current reviewed
binary and refresh the embedded Skill; leave `~/.collab/` and the running
daemon untouched. The global daemon is user-managed. Host reset and maintenance
are outside the agent interface.

## Production tmux sockets

The tmux launcher owns its sockets and lifecycle. A socket is an IPC address,
not a durable identity. Collab maintenance does not relocate, stop or restart
tmux. The path of an existing socket never blocks identity recovery.

Production CLI observation does not parse `TMUX` to inject a socket path. The
daemon observes the caller's actual pane and session through its known runtime
endpoints. cwd selects the project; the stable name owns the work and the
reported agent sessionID is its current execution carrier. tmux session does
not select identity. The observed socket and pane locate delivery only. A stale
environment value or historical pane binding cannot override that observation.

## Project discovery and centralized persistence

The project `.agent-collab/` marker discovers a project. It is not a second
runtime truth. Project journals, mailbox, tasks, claims and bindings persist
under the global host root's project/app namespace. The project cwd remains
the logical scope even though its data is stored beneath `~/.collab`.

The current runtime opens only the centralized project/app store. Legacy
project-local journals and mailbox files belong to the retired storage version;
they are not imported or replayed by registration, recovery or daemon startup.
They remain on disk. A stale route storage reference is rebound to the current
central store without copying old data. Existing central peers and messages
remain authoritative; a corrupt central journal still fails explicitly.
Do not copy or edit journals to repair an active service.
No command may reconstruct the current peer from a legacy
`runs/<worker-id>/identity.json`. AppSDK reset must never delete
`.agent-collab/`; do not delete files by hand or run migration to bypass a
registration error.

For a new project, use the installed `collab context`. Existing tasks and
messages belong to the storage owner; ordinary version installation or
identity recovery is not authorization to discard them.

## Identity and role

- The current client is Codex only. Names own project work; the registered
  agent sessionID is the current execution carrier. The current AppServer
  route uses its verified owner and exact agent sessionID/threadID pair.
  A tmux socket, pane, tmux session and process ID locate transport or probes;
  they do not select identity. Do not reconstruct an endpoint or edit routes.
- A Git worktree does not inherit `.agent-collab/` and is not an identity
  context. Run `collab context`; the CLI automatically observes runtime facts
  and the daemon owns identity selection, creation, recovery, update,
  credential/binding/lease persistence, route publication, and the complete
  snapshot. Historical routes are not candidates and `routes.jsonl` must not
  be read to guess one. Never register the worktree as a second peer or create
  a second route.
  An external linked worktree resolves to its registered Git main root;
  directory containment does not establish project identity. Do not switch cwd
  to recover an identity. A failed `collab context` is not evidence that no
  master exists. Master promotion requires explicit user approval and an
  authenticated current binding, not the incumbent's or the candidate's
  liveness.
- Default role is `peer`; master is explicit and user-approved.
- `collab context` is the single information endpoint for the current peer,
  binding, role, transport, liveness, tasks, master, and peers.
- `collab master status`, `collab who`, `collab status --all`, and
  `collab worker status` are read-only operator diagnostics. They are not agent
  identity-recovery steps.
- If `collab context` reports `requires_identity_update`, read
  `required_fields`, `reason`, and `exact_error`. Supply only the requested
  AppServer `session_id`, `thread_id`, `endpoint`, or `namespace` values for this invocation through
  `collab context --provide`. The daemon completes identity selection,
  recovery, registration, credential persistence, binding, and lease state.
  Explicit conflicts and errors preserve their original error.

## Transport selection

- Workers never choose transport themselves. The daemon validates runtime facts
  and selects the channel.
- Initial registration prefers a verified current tmux pane, and the daemon
  selects transport according to runtime facts. Context reconciles the current
  explicit capability with the durable binding. Supplying a current Desktop
  bridge capability is an explicit transport update. An RPC error never selects
  another transport or an old endpoint; tmux is not a native communication,
  wake, presence, or status fallback.
- Do not treat pane presence as evidence of Codex turn state or message
  consumption.

## Registration verification

Run `collab context`. It resolves the canonical project root, observes the
available runtime facts, and asks the daemon to establish or recover the
identity. If the snapshot contains `requires_identity_update`, supply only the
requested current factual fields in this invocation. Every AppServer context must
carry current session/thread/endpoint/namespace from the host environment or
`--provide`; historical registration cannot fill missing current facts.
MCP callers submit current facts through `provide` on each call. `--query` is
only a retained-operation read and needs no current endpoint:

```sh
collab context --provide '{"session_id":"...","thread_id":"...","endpoint":"...","namespace":"..."}'
```

For the AppServer branch, the factual supplement uses `session_id`, `thread_id`,
`endpoint`, and `namespace`. Desktop bridge instead obtains the current scoped
capability and real caller metadata through its normal managed MCP entry.
Do not supply a tools pipe as an AppServer endpoint or copy an old caller turn.
The agent sessionID still identifies the execution carrier independently of
the native thread. The supplement does not accept `worker_id`, approval, token,
generation, binding, or project scope. Do not run `appsdk init`, run
`collab init` as a substitute for `collab context`, run a manual identity
recovery command, or start a status/route/archive hunt to repair identity. A
conflicting claim on your tmux pane (stale, or from another worker or project)
is replaced by the daemon inside the same `collab context` call, so it is not a
case you adjudicate. If `collab context` fails with an explicit conflict or
error, preserve the exact error and stop. Never edit `routes.jsonl`,
`server.pid`, journal, mailbox, or identity files to make registration appear
healthy.

## Read-only route diagnostics

A registered AppServer route is live only when the verified owner resolves the
exact saved session/thread and project cwd through native `thread/read`. A
tmux-only binding uses current persisted worker/binding/pane ownership for
identity `Present` and validity, without live probe. Route reads and restoration
use that ownership. A missing endpoint is absent; a failed status probe is
unknown. Neither registered identity nor endpoint presence proves agent
activity, transport submission or message consumption.

An operator may use these read-only diagnostics for audit:

```sh
collab route resolve --native-thread-id <thread-id> --session-id <session-id>
collab worker status <peer-id>
collab status --all
```

Route resolution compares supplied runtime IDs with the current registered
binding and never selects another thread by history. It is read-only and returns
no token. The daemon remains the sole owner of route selection and identity
recovery. Never edit `routes.jsonl`, identity files, journal, or mailbox to force
a route, and never start a second daemon.

`collab route resolve` accepts `--native-thread-id <id>` and `--session-id <id>`,
which must match the live runtime binding. It returns exactly one route,
`ROUTE_RESOLVE_NOT_FOUND` for zero matches, and `ROUTE_RESOLVE_AMBIGUOUS` when
multiple current bindings match. Missing, unknown, mismatched or malformed
endpoints fail explicitly.
