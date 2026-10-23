# Context-Owned Migration and User-Managed Daemon Boundary

## Identity recovery includes legacy migration

Use `collab context` in the current project. Context owns compatibility with
historical identity records, current name/session selection, registration,
credential persistence, stale route retirement and communication binding.
Agents do not inspect archives to choose an identity, close historical workers,
copy tokens, edit control files or run a separate migration before recovery.

The stable name owns work. The agent sessionID is its current execution carrier.
Pane, tmux session, socket and server PID only locate delivery and observation.
They do not establish identity ownership. A later committed registration for the
same project, app and agent session replaces the previous carrier. It must not
stop an old process or change another session's identity.

An explicit approved recovery target is resolved before historical ambiguity.
Historical bindings cannot veto that authorization. Context supplies its own
current scope and incumbent facts; agents do not invent fences, timestamps or
archive choices. Existing approval remains valid for the authorized operation.
For commands and online takeover confirmation, see
[stable-name recovery](stable-name-recovery.md#恢复操作context-自己迁移agent-不做历史清理).

Context must converge legacy duplicates itself. It preserves name-owned open
tasks, resources and unconsumed messages and updates their execution binding.
Consumed messages and closed-task history remain historical. Migration does not
grant master authority without the user's explicit master restoration approval.

Every new AppServer context must carry current session/thread/endpoint/namespace
from the calling host or this invocation's `--provide` (MCP: `provide` on each
call). Desktop uses its current scoped bridge capability, including through
CLI `--provide`; agent-side MCP calls are optional. The installed MCP bridge
can obtain that capability from real executor metadata. Desktop does not
require an AppServer endpoint or borrow the managed
AppServer socket. Historical registration never supplies missing current facts. Missing
facts preserve identity and work and return `missing_facts`. If user intent is required, supply that intent
through the context approval interface. Query an existing operation with
`collab context --query --op <operation_id>`; a query does not perform migration.
Pending or requested-input results are not successful recovery. Completion must
include a successful registration receipt and current communication binding.

A raw historical ambiguity after valid approval is a runtime defect. Preserve
the operation result and report the missing implementation or inactive version.
Do not reinterpret it as missing user authorization, ask the agent to choose an
archive, or discard project state to make recovery succeed. Real persistence,
I/O and current endpoint errors remain explicit.

## Persistence owners

The installed binary resolves host-wide Collab state under `$HOME/.collab`;
`COLLAB_STATE_DIR` is for isolated tests. This owner includes identity, routes,
mailboxes and daemon runtime state. Project-local `.agent-collab/` may be a
discovery or legacy input. Context and the server own its compatibility; it is
not a second independent host truth. AppSDK `.appsdk/` and `.appsdk-control/`
belong to AppSDK and are outside Collab recovery.

Never manually delete journals, mailbox files, identity tokens or route state.
Do not use `collab migrate inspect/plan/apply/verify` as an ordinary identity
recovery sequence. Those operator storage-migration tools do not transfer
context's migration responsibility to an agent.

## Install and activate separately

Use the formal installer from the reviewed current source:

```sh
scripts/install-global-collab.sh
```

It builds and installs Collab and refreshes the embedded Skill. It does not
activate the new global daemon or discard durable state. Verify the installed
version and affected public behavior. Report installed and live versions
separately; installation alone does not prove live identity or communication
recovery. If an old binary shadows the installed binary, resolve that exact
installation or command-cache problem. Do not retain an old runtime as a
recovery fallback.

One supported Desktop bridge service entry is the installed `~/.cargo/bin/collab-desktop-mcp`.
Configure this command through the normal Desktop MCP configuration and load
it through the host's normal mechanism. It proxies Collab tools and publishes
a scoped bridge on each new `collab_context` call from real executor metadata.
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

Registration/recovery uses the live bridge's scoped current capability. With
a genuinely enrolled bridge, ordinary `collab context` reads its exact current
declaration automatically. No manual endpoint or agent-side MCP call is needed.
The daemon verifies the declared candidate through its existing adapter.
An actual verified current tmux pane retains its existing admission priority;
an inherited Desktop environment marker does not make unused bridge facts
mandatory. A pane environment string alone cannot establish that placement.
For an explicit current capability supplied by the host, the CLI also accepts
the shape below. Use only actual current values; never copy historical paths:

```sh
collab context --provide '{"session_id":"<current agent sessionID>","thread_id":"<current native thread ID>","desktop_bridge":{"endpoint":"<current bridge unix:// endpoint>","namespace":"codex_app","session_id":"<current agent sessionID>","thread_id":"<current native thread ID>","cwd":"<canonical project cwd>"}}'
```

The bridge retains its real native caller context. CLI parameters do not
invent a turn ID, authorize an arbitrary target or connect the daemon directly
to the Desktop tools pipe. `desktop_bridge.endpoint` is the bridge UDS;
top-level `endpoint` is the separate AppServer field and is not its replacement.
Normal restoration preserves the name and role and lets context update routes,
valid reception subscriptions and open-task session bindings.

The declaration is current bridge-owned control, not a persisted identity
record. Discovery matches canonical invocation cwd, current agent sessionID
and current native thread ID. It does not search sockets, reuse old routes,
infer caller turns, or alter names. Explicit transport input keeps its own
path; queries and ordinary identity reads do not discover or update transport.
An exited bridge can leave inactive `current.json` metadata. The daemon still
verifies actual native capability; a genuine replacement publishes over it
without an identity takeover or stale-lock cleanup by the agent.

If no bridge has genuine executor enrollment, no current capability can be
published. Return the precise Desktop capability error. Do not ask for an
AppServer endpoint, require an MCP tool invocation, guess a socket, edit
identity state or restart a service. Discovery of an enrolled bridge does not
prove unattended first bootstrap of a new chat. A task-owned foreground bridge
experiment proves only that experiment's lifetime. Report source, installed
publisher/CLI loading and actual Desktop delivery separately.

`sendmessage` returns a durable message ID; native adapter acceptance is a
separate fact. Confirm actual arrival in the target chat. Run `collab recv`
once for that notification; it consumes and prints the durable messages and
needs no follow-up ACK. `collab inbox` and `collab msg <id>` only inspect.
A reply uses `collab sendmessage` to the original sender when authorized.
Queue acceptance, actual arrival, consumption and reply are separate evidence.

Collab has one global user-managed daemon. Agents never start, stop, restart or
kill it, request maintenance credentials, or create a second global runtime.
User maintenance commands and credentials are outside the agent interface.
When the service is unavailable, preserve durable state and report that fact.
Project replay failure must not disable authenticated user maintenance: its
credential check and shutdown path belong to the host, independently of the
project reducer. Agents report the failure and do not edit journals or acquire
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
for context recovery, an identity-conflict remedy or a history-preserving
migration. Do not imitate reset by editing files.

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
