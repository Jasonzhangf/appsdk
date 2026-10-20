# Migration and User-Managed Daemon Boundary

## Global truth and project-local state

The host-wide runtime truth is the Collab state root resolved by the installed
binary:

```text
$COLLAB_STATE_DIR for isolated tests, else $HOME/.collab
  server.sock
  daemon.lock
  server.pid
  events.jsonl
  log.txt
```

Each registered project keeps its own `.agent-collab/` reducer input and
project-scoped journal, mailbox projection, tasks, claims, bindings, and
worktrees. It is project-local durable state, not a disposable cache and not
the host-wide runtime socket. The global `~/.collab/` state owns the host
socket, route table, and global identity/liveness records. AppSDK governance
has a separate truth owner and reset transaction; neither owner may delete or
rewrite the other's state.

For a new or explicitly authorized clean project, do not treat an existing
project-local `.appsdk/`, `.appsdk-control/`, or `.agent-collab/` directory as
the new version's baseline. The current global binary is the only runtime
baseline. AppSDK removal belongs to AppSDK's reset owner; Collab removal belongs
to the Collab reset/migration owner. Do not migrate or replay legacy local
history merely to make initialization look clean.

## Version upgrade and local installation cleanup

Use the repository's install sequence from the reviewed current source:

```sh
scripts/install-global-collab.sh
```

The installer performs one versioned release build and installs those exact
binaries under `$CARGO_HOME/bin`; the exact new binary refreshes the embedded
Skill. It never removes `~/.collab/`, a project's `.agent-collab/`, AppSDK
state, business source, or evidence. Legacy user-local copies are not removed
automatically: first prove an exact copy is Collab from its own version
response, then remove only that verified pair. Unverified path collisions
remain untouched and must be reported.

The upgrade is a replacement, not a compatibility layer:

- Build and install from the reviewed `origin/main`; use only the installed
  `$CARGO_HOME/bin/collab` and `$CARGO_HOME/bin/collab-mcp` for subsequent
  checks.
- Do not migrate, interpret, or read old local Collab control history merely
  to keep an old project alive. For an explicitly authorized clean epoch, the
  current project master may use `collab reset --project`; for replayable
  state, use `collab migrate`.
- If a verified legacy user-local Collab binary pair exists, remove only that
  exact pair after the canonical pair is installed. Do not remove project
  `.agent-collab/`, `~/.collab/`, AppSDK state, or business source.
- If the installed path still resolves an old binary, fix `PATH` or the shell
  command cache and verify `command -v` and `--version`; never keep an older
  binary as a fallback.
- After the installed version is current, verify `collab status --all`,
  `collab context`, and one live tmux send/recv receipt replay. Binary
  installation alone does not prove the running daemon or route is current.

Collab has one global user-managed daemon. Agents never start, stop or restart
it, including for identity recovery or installation. User maintenance commands
and credentials are outside the agent interface. Preserve identity and journals
when the service is unavailable. Never use broad process kills, a second socket
or an older binary as a fallback.

## Formal v1 migration

```text
inspect -> plan -> admission freeze -> snapshot
        -> install reviewed binaries with scripts/install-global-collab.sh
        -> user-managed runtime activation, if required
        -> identity rebind -> verify -> resume
```

```sh
collab migrate inspect
collab migrate plan
collab migrate apply
# installation does not change the user-managed running service
collab migrate verify
```

Migration authenticates the initiator, holds one transaction lease, inspects
legacy fields/waits/bindings/journal/counts/single-writer state, freezes after a
deterministic snapshot, and verifies identity rebind plus snapshot/count
continuity before resume. Unresolved owner/count mismatch requires an explicit
operator decision; never invent an owner.

Never delete/recreate `.agent-collab`, edit its truth files, clear
evidence/mailbox, copy identity tokens, reset bindings, or mix old/new writers.

## Legacy project migration or retirement

Use this path when an existing project has `.agent-collab/` from an older
version and the operator wants to move to the current Collab baseline:

```sh
cd /abs/path/project
collab migrate inspect
collab migrate plan
collab migrate apply
# install the reviewed Collab binary
collab context
collab migrate verify
```

`inspect` is read-only. `plan` does not freeze admission. `apply` freezes
admission and persists the deterministic snapshot. `verify` resumes admission
only after journal, mailbox, task, identity, and count continuity pass.

Run the same operation from the project root whose `.agent-collab/` is being
migrated. Do not run it in a worktree that merely points at another project's
scope, and do not start a second daemon for the migration.

If the result is `reset_required`, `needs_operator`, `unknown`, a malformed
journal, a count mismatch, or an unresolved owner:

1. Preserve the exact error, migration ID, snapshot, and source bytes.
2. Stop admission and dependent writes.
3. Resolve the named owner or blocker through the Collab server protocol.
4. Create a new plan only after the source can be proved safe.

When the current project master explicitly decides to abandon the old epoch
instead of preserving it, and the project-local journal cannot be replayed
under the current contract, run the online project reset from the current
project:

```sh
collab reset --project
```

`collab reset` is not `collab migrate`. It never imports history, never
pretends to preserve counts, and never claims delivery, review, or install
evidence. It deletes the current project's historical messages, routes, tasks,
assets and load while preserving other projects and the user-managed global
daemon. There is no offline prerequisite or archive prerequisite. Historical
pane owners and open work do not block the reset. Real persistence, I/O or input
safety errors remain explicit. No legacy discard mode or separate approval
argument exists for this project reset.

`.appsdk/` and `.appsdk-control/` are AppSDK-owned and are never removed by
this command. The project-local `.agent-collab/` is a project scope input,
while the host-wide runtime truth remains `~/.collab/` (`server.sock`,
`events.jsonl`, `log.txt`, and route state). Do not manually remove
`.agent-collab/server/journal.jsonl`, mailbox files, binding records or route
state to imitate reset.

Never manually delete `.agent-collab/`, edit journal or mailbox JSON, clear
the mailbox, or copy identity tokens. Do not convert a failed migration into a
fresh registration by deleting the source outside this command. AppSDK
governance reset remains a separate owner and transaction; it never removes
`.agent-collab/`.

After `verify`, record the migration record ID, source and target versions,
preserved counts and identity rebind. A restored peer and a restored master
must each send, receive and reply through the current route for live acceptance.
Migration verification does not prove AppSDK delivery, review, install, or
freeze.

## User-managed daemon boundary

Agents do not perform daemon lifecycle actions or request credentials for them.
Installation does not activate a new runtime. Report installed and live states
separately. Restore peer identity with `collab context` against the existing
service. Host reset and maintenance are outside the agent interface.

### Native endpoint diagnostics

`collab context` is the only agent identity entry. If it reports
`requires_identity_update`, an agent supplies only the requested factual fields
once through `collab context --provide`. The daemon validates the current
session/thread or tmux anchor and owns identity selection, creation, recovery,
update, registration, credential persistence, and binding. Read-only route and
worker diagnostics are operator tools, not an agent recovery sequence.
Notifications require the native AppServer route to pass self-check; do not
switch that binding to tmux on failure. See
[`state-paths.md`](state-paths.md#read-only-route-diagnostics).

## Deprecated commands

Never use implicit first-register master, master recovery, transfer-master,
legacy central dispatch (`collab task dispatch`), or heartbeat recovery.
Ordinary peer assignment uses the public board publish/claim/invite workflow.
`collab subagent dispatch` and managed-subagent records are legacy compatibility
for diagnosis or migration only; they grant no new production execution
authority and are not a fallback after board refusal or admission failure.
Collab master is a
separate explicit user-approved authority
decision, not Codex root, migration, or daemon recovery. The authority owner is
the current typed grant in the exact project and app scope, with only empty or
assigned states; the current holder controls independent of its liveness. An
approved `collab master promote` replaces any recorded holder, only the current
master grant holder may delegate, and an authenticated peer with explicit
approval may `collab master clear` the scoped grant.
Journal `RootAssigned` events become `MasterAssigned` on daemon replay.
Hidden `collab root ...` commands run the same master protocol. AppSDK init and
fresh governance init never clear master authority and are not an implicit
authority reset. Independent peers may decline a master collaboration invite.
Master-only rules require an explicitly authorized master grant. Legacy managed
records do not create execution authority:

```text
collab master status
collab master promote --approval "<user text>"
collab master clear --approval "<user text>"
collab master delegate <peer>
```

Deprecated:

```text
collab role
collab master recover
collab transfer-master
collab task claim
collab task dispatch
collab remove-worker
```
