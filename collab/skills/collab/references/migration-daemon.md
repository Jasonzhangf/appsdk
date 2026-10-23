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
call). Desktop bridge uses its normal MCP entry: the executor supplies real
caller metadata and the managed bridge supplies its current scoped UDS
capability. It does not require an AppServer endpoint or borrow the managed
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

Desktop's Collab MCP entry is the installed `~/.cargo/bin/collab-desktop-mcp`.
Configure this command through the normal Desktop MCP configuration and load
it through the host's normal mechanism. It proxies Collab tools and publishes
a scoped bridge on each new `collab_context` call from real executor metadata.
Use `project_scope` for project selection; `provide` may contain the current
agent `session_id` when needed. Do not configure a tools-pipe path or caller
turn. An old `collab-mcp` process does not gain bridge support from a daemon
reload. Verify MCP activation separately from daemon activation.

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
