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

For an explicitly authorized reset, name the level. Exactly one level is
required, and `--storage-root` is required for `--host`
(`RESET_STORAGE_ROOT_REQUIRED`):

```sh
collab down
# project control plane; run from the project root
collab reset --project --discard-legacy --approval "<explicit user authorization>"
# host control plane
collab reset --host --storage-root <project root> --discard-legacy \
  --approval "<explicit user authorization>"
collab up
collab status --all
```

The routes level was removed: one pane owns one route binding host-wide
(`docs/design/collab-pane-route-ownership-20261006.md`).

`--host` keeps `reset.jsonl` and `archives/`, which are the audit trail, and it
keeps the project business payload under `.agent-collab/`. Retire that payload
with `--project`.

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

- The current client is Codex only. The current route is bound to the verified
  AppServer owner and exact sessionID/threadID pair. The daemon may use a
  verified tmux socket/server/session/pane/process tuple when both runtime IDs
  are unavailable. Do not reconstruct an endpoint from a stale route or
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

- Workers never choose transport themselves. The server validates the candidate
  and selects the channel.
- AppServer RPC is preferred when its owner passes native thread/session/cwd
  self-checks. A tmux-only candidate can be selected as its own transport when
  no AppServer candidate is available. Once an AppServer binding is selected,
  tmux is never a communication, wake, presence, or status fallback.
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
generation, binding, or project scope. Do not run `appsdk init`, `collab init`,
a manual identity recovery command, or a status/route hunt to repair identity. If
`collab context` fails with an explicit conflict or error, preserve the exact
error and stop. Never edit `routes.jsonl`, `server.pid`, journal, mailbox, or
identity files to make registration appear healthy.

## Read-only route diagnostics

A registered AppServer route is live only when the verified owner resolves the
exact saved session/thread and project cwd through native `thread/read`. A
tmux-only binding uses its verified pane probe as that transport's liveness.
A missing endpoint is absent; a failed probe is unknown. Neither presence signal
establishes message consumption.

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
