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

- `server.sock` belongs to the global user-managed daemon. Agent commands do not
  create or remove it. Never write to it by hand.
- `daemon.lock` prevents a second daemon. Do not delete or edit it.
- `server.pid` is the current daemon PID. Read it through `collab context` or
  `collab status --all`; do not use it to control the user-managed service.
- `events.jsonl` is the append-only durable daemon event stream. Do not edit or
  delete it.
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

Production tmux sockets also belong in a stable, private user directory, not
`/tmp`, `/private/tmp`, or the OS temporary directory. The tmux launcher owns
this setting. Collab observes the current socket through `TMUX` and uses that
exact address; it does not create or relocate the production tmux server.

For shell-launched tmux, create the directory once:

```sh
install -d -m 700 "$HOME/.tmux/run"
```

Export this setting in the startup file used by the launcher (`~/.zshenv` for
zsh, including noninteractive shells):

```sh
export TMUX_TMPDIR="$HOME/.tmux/run"
```

A new tmux server then uses `$HOME/.tmux/run/tmux-<uid>/default`. `-L <name>`
changes the socket name within that directory. An explicit `-S <path>` bypasses
this default and must itself use a stable user path. GUI and service launchers
that bypass shell startup must configure their own launch environment. Do not
claim that a shell edit changes those launchers.

Verify a new, isolated server through `display-message -p '#{socket_path}'`.
The socket is an IPC endpoint; a stable pathname does not preserve processes or
sessions after a reboot.

This setting affects future server creation. Existing servers keep their actual
socket paths. Clients inside an existing tmux session may still select its
socket through `TMUX`; outside clients can use its observed `-S` address until
the user migrates that server. Do not move a live socket, substitute a symlink,
rewrite Collab routes to an unobserved address, or reject current identity
recovery because an existing server still uses a temporary path. Changing the
default alone is not live migration. Use `collab context` to restore identity
and communication from the real current endpoint.

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

- The current client is Codex only. The current route is bound to the verified
  AppServer owner and exact sessionID/threadID pair. The daemon may use a
  observed tmux socket + pane anchor independently of runtime IDs. Session and
  process IDs remain observations. Do not reconstruct a new endpoint or
  manually edit route state.
- A Git worktree does not inherit `.agent-collab/` and is not an identity
  context. Run `collab context`; the CLI automatically observes runtime facts
  and the daemon owns identity selection, creation, recovery, update,
  credential/binding/lease persistence, route publication, and the complete
  snapshot. Historical routes are not candidates and `routes.jsonl` must not
  be read to guess one. Never register the worktree as a second peer or create
  a second route.
- Default role is `peer`; master is explicit and user-approved.
- `collab context` is the single information endpoint for the current peer,
  binding, role, transport, liveness, tasks, master, and peers.
- `collab master status`, `collab who`, `collab status --all`, and
  `collab worker status` are read-only operator diagnostics. They are not agent
  identity-recovery steps.
- If `collab context` reports `requires_identity_update`, read
  `required_fields`, `reason`, and `exact_error`. Supply only the requested
  `session_id`, `thread_id`, `endpoint`, or `namespace` values once through
  `collab context --provide`. The daemon completes identity selection,
  recovery, registration, credential persistence, binding, and lease state.
  Explicit conflicts and errors preserve their original error.

## Transport selection

- Workers never choose transport themselves. The daemon validates runtime facts
  and selects the channel.
- Initial registration prefers a verified current tmux pane, and the daemon
  selects transport according to runtime facts. Persisted restoration reuses the
  durable selected binding. Once an AppServer binding is selected, tmux is never
  a communication, wake, presence, or status fallback.
- Do not treat pane presence as evidence of Codex turn state or message
  consumption.

## Registration verification

Run `collab context`. It resolves the canonical project root, observes the
available runtime facts, and asks the daemon to establish or recover the
identity. If the snapshot contains `requires_identity_update`, supply only the
requested factual fields once:

```sh
collab context --provide '{"session_id":"...","thread_id":"...","endpoint":"...","namespace":"..."}'
```

The supplement accepts only the four scalar keys `session_id`, `thread_id`,
`endpoint`, and `namespace`. It does not accept `worker_id`, approval, token,
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
