# Current Identity and User-Managed Daemon Boundary

## Choose the migration owner

| Trigger | Operator, command and working directory | Completion evidence |
| --- | --- | --- |
| Current Collab session or transport changed, or the name has an explicit recovery request | Affected agent runs `collab context` from its actual project in the original host session. Read the [recovery decision table](stable-name-recovery.md#table-d-context-recovery-and-current-actions) before supplying fields | Same stable name and applicable role, current verified binding, existing work and unread messages still owned by that name |
| Current Collab binary or daemon is behind the delivered version | Authorized installer operator uses the reviewed AppSDK checkout; the user activates from the actual project as described below | Installed CLI and running daemon both report the intended version; verify affected behavior separately |
| Current AppSDK project needs its managed governance resources refreshed | Project owner runs `appsdk init` from an existing governed root with `.appsdk/project.json`, then `appsdk verify`. For first adoption or relocation use the confirmed `appsdk prepare` path in the AppSDK governance Skill | Existing project and business records stay in place; verify the affected entry in the consumer |
| User explicitly requests discarding one project's Collab state | Current project master runs `collab reset --project` from that exact project after confirming the discard scope | Read the reset receipt for that project. Use the identity recovery path separately if the current session needs to bind |

For a healthy registered session with an active goal, continue the existing
schedule across an ordinary version upgrade. Use goal and notification status
if a concrete fact is missing. Archived project-local Collab data has no
manual import path. When a request depends on data found only in retired
storage, report the specific missing current record and preserve the source.

## Context uses the current identity map

Use `collab context` in the current project only for registration or evidenced
identity recovery. Ordinary task progress uses `collab task status '<id>'`, and
identity inspection uses `collab context --query`. Identity selection reads only
`identity-map.json`. Exact credentials and current grants serve that selected
principal. Registration, name/session updates and communication bindings use
the same map owner. Agents do not choose an archive, copy tokens or edit state.

The stable name owns work. The agent sessionID is its current execution carrier.
Pane, tmux session, socket and server PID only locate delivery and observation.
They do not establish identity ownership. A later committed registration for the
same project, app and agent session replaces the previous carrier. It must not
stop an old process or change another session's identity.

An explicit authorized recovery target is applied through the current map.
Old bindings and names cannot veto that authorization. Missing map rows permit
first registration with current facts; a corrupt map returns its real error.
For all current-action branches, see the [recovery decision table](stable-name-recovery.md#table-d-context-recovery-and-current-actions).

Context preserves name-owned open tasks, resources and unread messages while
updating their carrier. Consumed messages and closed-task records stay intact.
Peer recovery does not grant master authority.

Recovery does not change accepted goal ownership or require another human
approval for authorized work. Reconcile finite goal/task schedules under
[notifications](notifications.md#patrol-duties-and-finite-schedules). Standing
project patrol and task-scoped cancellations remain owned by
[board-idle patrol](board-idle-patrol.md#table-c-patrol-wakes-and-cancellation), not by identity restoration.

First registration requires the selected host's real current facts. Desktop
uses its scoped bridge capability; it does not require tmux or borrow a managed
AppServer socket. DSH uses its actual gateway session and enrolled transport.
Supply only requested factual fields through `--provide` when necessary.
`collab context --query` projects current persisted state without registration,
consumption or recovery effects. Pending, missing facts and unknown communication
do not mean completed recovery.

A rejection based only on old history after valid authorization is a runtime
defect. Preserve the error and report the affected implementation or inactive
version. Do not ask for an archive choice or discard current work. Real I/O,
persistence and current endpoint errors stay explicit.

## Persistence owners

The installed binary resolves host-wide Collab state under `$HOME/.collab`;
`COLLAB_STATE_DIR` is for isolated tests. This owner includes identity, routes,
mailboxes and daemon runtime state. Project-local `.agent-collab/` may provide
project discovery. Identity comes only from the current identity-map.json;
business state loads directly from current state.json. No journal reader,
writer, replay, repair or migration maintenance API exists. Project-local
state is not a second host truth. AppSDK `.appsdk/` and `.appsdk-control/`
belong to AppSDK and are outside Collab recovery.

Never manually edit current identity, mailbox, token or route state. The removed
`collab migrate` command and MCP tool are not recovery or maintenance options.

## Install and activate separately

Use the formal installer from the reviewed current source:

```sh
scripts/install-global-collab.sh
```

Use the authorization already granted for the target installation. If the
installer returns `outcome=pending_user_activation` and `required_action=daemon_version_differs`,
read `~/.cargo/bin/collab --version` and `~/.cargo/bin/collab status` to confirm
the installed CLI and current `daemon_version`. Other installer failures use
the original error, failed phase and rollback receipt.

When activation or restart is needed after installing or upgrading `collab` outside protected TCC folders, the user or an authorized agent runs `~/.cargo/bin/collab up` (or `~/.cargo/bin/collab up --restart` when forcing replacement of a healthy same-version detached daemon) from the project root (or a registered worktree). Run `~/.cargo/bin/collab status` after `collab up` to confirm that `daemon_version` matches `~/.cargo/bin/collab --version`. Run `collab context` when resuming agent work on the restarted daemon. `collab down` and maintenance passwords remain user-only (`USER_MAINTENANCE_REQUIRED`).

The installer builds and installs Collab and refreshes the embedded Skill. It does not
activate the new global daemon or discard durable state. Verify the installed
version and affected public behavior. Report installed and live versions
separately; installation alone does not prove live identity or communication
recovery. If an old binary shadows the installed binary, resolve that exact
installation or command-cache problem. Do not retain an old runtime as a
recovery fallback.

One supported Desktop bridge service entry is the installed `~/.cargo/bin/collab-desktop-mcp`.
Configure this command through the normal Desktop MCP configuration and load
it through the host's normal mechanism. It proxies Collab tools and ensures
the shared runtime bridge. Public CLI registration can ensure the same service
and enroll a new chat without an MCP tool call.
The MCP config must pass through `CODEX_APP_TOOLS_PIPE_PATH`,
`CODEX_MCP_NODE_PATH`, `CODEX_SESSION_ID` and `CODEX_INTERNAL_ORIGINATOR_OVERRIDE`
via `env_vars`; do not replace host-provided values with copied paths or IDs.
Use `project_scope` for project selection; `provide` may contain the current
agent `session_id` when needed. Do not configure a tools-pipe path or caller
turn. An old `collab-mcp` process does not gain bridge support from a daemon
reload. Verify service activation separately from daemon activation. Check
project-local `.codex/config.toml` as well as global configuration: an obsolete
project override can select direct `collab-mcp` despite a correct global entry.
Do not assume Desktop exposes a reconnect button, ask agents to find one, or
prescribe another app restart without identifying the actual stale owner.

## Desktop CLI registration and messaging

Desktop's identity anchor is the current agent sessionID, supplied by the
calling runtime (for Codex, `CODEX_SESSION_ID`). cwd selects the project; the
stable name owns its goal, tasks, resources and unread messages. Native thread
ID, bridge endpoint, pane and tmux session only locate communication. An equal
session/thread value in a sample does not make these fields interchangeable.
Changing a thread or bridge endpoint must not allocate a new name or role.
Same-name session changes follow the existing name-recovery contract.

Use the CLI in the project directory:

```sh
collab context
collab sendmessage --to <registered-recipient> --subject <topic> "<message>"
collab recv
```

Registration/recovery uses one shared bridge service per Desktop runtime.
`collab context` automatically connects to that service and enrolls the current
session and native thread. The service verifies the thread and project with
Desktop before publishing the scoped capability. A new chat needs no earlier
MCP invocation, manual endpoint or bridge setup. Closing an MCP wrapper does
not stop the runtime service. The service exits when its Desktop host pipe closes.
An explicitly supplied AppServer endpoint, including the calling host's
endpoint environment, retains its direct AppServer path and skips discovery.
The daemon verifies the declared candidate through its existing adapter.
An actual verified current tmux pane retains its existing admission priority;
an inherited Desktop environment marker does not make unused bridge facts
mandatory. A pane environment string alone cannot establish that placement.
For an explicit current capability supplied by the host, the CLI also accepts
the shape below. Use only actual current values; never copy historical paths:

```sh
collab context --provide '{"session_id":"<current agent sessionID>","thread_id":"<current native thread ID>","desktop_bridge":{"endpoint":"<current bridge unix:// endpoint>","namespace":"codex_app","session_id":"<current agent sessionID>","thread_id":"<current native thread ID>","cwd":"<canonical project cwd>"}}'
```

The bridge uses a clearly labelled service correlation ID for native calls.
That ID does not claim that an agent execution turn occurred and is not an
authorization credential. Desktop still verifies the native thread; enrollment
must match its canonical project. CLI parameters do not authorize an arbitrary
target or connect the daemon directly to the Desktop tools pipe.
`desktop_bridge.endpoint` is the bridge UDS;
top-level `endpoint` is the separate AppServer field and is not its replacement.
Normal restoration preserves the name and role and lets context update routes,
valid reception subscriptions and open-task session bindings.

The declaration is current bridge-owned control, not a persisted identity
record. Discovery matches canonical invocation cwd, current agent sessionID
and current native thread ID. It does not search sockets, reuse old routes,
infer execution turns, or alter names. Explicit transport input keeps its own
path; queries and ordinary identity reads do not discover or update transport.
An exited bridge can leave inactive `current.json` metadata. The daemon still
verifies actual native capability; a genuine replacement publishes over it
without an identity takeover or stale-lock cleanup by the agent.

If the runtime service cannot connect or Desktop refuses the thread/project,
return that precise error and retain identity and messages. Do not ask the agent
for an AppServer endpoint or an MCP invocation, guess a socket, delete state,
or edit identity state. Report source, global installation and actual Desktop
delivery separately. Service restart restores persisted bindings after native
verification; agents do not repeat registration to repair a bridge process.

`sendmessage` returns a durable message ID; native adapter acceptance is a
separate fact. Confirm actual arrival in the target chat. Run `collab recv`
once for that notification; it consumes and prints the durable messages and
needs no follow-up ACK. `collab inbox` and `collab msg <id>` only inspect.
A reply uses `collab sendmessage` to the original sender when authorized.
Queue acceptance, actual arrival, consumption and reply are separate evidence.

Collab has one global daemon shared across projects. Agents may run `collab up` to start or restart the daemon onto the installed binary (preserving durable routes, identities, mailbox, and board state), but never run `collab down`, request maintenance passwords, or create a second global runtime.
User maintenance commands and credentials are outside the agent interface.
When the service is unavailable, preserve durable state and report that fact.
Project state read failure must not disable authenticated user maintenance: its
credential check and shutdown path belong to the host, independently of the
project state owner. Agents report the failure and do not edit control files or acquire
maintenance authority to compensate.
AppSDK/Collab maintenance never requires tmux server lifecycle operations or
termination of the host pane.

## Reset is explicit discard, not recovery

Only use a project reset when the user explicitly requests discarding that
project's Collab epoch and the caller holds the current project master grant:

```sh
collab reset --project
```

It clears this project's messages, routes, tasks, assets and load. It preserves
other projects, AppSDK state and the global daemon. It is not a prerequisite
for context recovery or an identity-conflict remedy. Do not imitate reset by
editing files.

## Master and deprecated interfaces

Master is an explicit scoped authority. Context may restore it with the user's
approved master intent. Peer recovery alone does not promote the caller.
The supported separate authority interface is:

```text
collab master status
collab master promote --approval "<user text>"
collab master clear --approval "<user text>"
collab master delegate <peer>
```

Do not use legacy `collab role`, `collab master recover`,
`collab transfer-master`, `collab task claim`, `collab task dispatch` or
`collab remove-worker` as recovery steps. Managed-subagent compatibility
records do not grant execution authority. Master and peer both orchestrate
long-horizon goals at different levels; one-shot execution goes to subagents.
