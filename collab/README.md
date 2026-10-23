# collab

This directory is the canonical Collab source and build owner inside the
AppSDK repository. Collab is compiled as the separate `collab` and
`collab-mcp` binaries, but it is not maintained or released from a separate
repository. Review, merge, push, install, and daemon restart all use the
containing AppSDK `main`.

Project-local coordination for independent coding agents. One Rust daemon owns
the append-only journal, durable mailbox, task/resource state, and migration
transaction. The server selects the App Server transport. Each subscribed
wake is a bounded preview; the durable mailbox remains authoritative.

## tmux socket configuration

Collab observes tmux panes only through host-owned socket endpoints. The
daemon state root also accepts a `tmux-endpoints.json` capability file. For a
host-wide setting, list the sockets in `~/.appsdk/config.toml`:

```toml
[tmux]
endpoints = ["/Users/me/.tmux/run/tmux-501/default"]
```

List both old and new sockets during a migration. Collab matches the caller's
live pane ancestry, so a pane selects its own server without a caller-supplied
socket path. Collab never reads the socket from the `TMUX` environment value.

## Model

- Every registered identity is an equal peer. Declared `master`/`worker` roles
  are removed.
- Registration returns the `role_brief` effective at that registration.
  `collab context`, `collab who`, and worker status project the current role
  brief from server role truth; promotion and delegation return the new brief.
  The brief defines role, authority, derivation, blocked boundary, completion
  action, and next action without a second prompt contract.
- Registered masters and peers publish pending work on the board. A peer may
  publish and claim its own work, and the master may invite a suitable peer.
  Legacy managed-subagent records preserve historical facts only; they grant
  no new production execution authority.
- Each peer owns its task lifecycle after acceptance: latest-main sync, private
  worktree, implementation, tests, exact commit, candidate verification, short
  integration lease, main merge/verification/push, and cleanup.
- Peers communicate through an explicit `sendmessage` with a required short
  subject and durable body. Use
  it for resource occupancy/release or a direct peer notice; it never creates
  an inferred task, progress, ACK, or continuation loop.
- Registration creates one seven-day reusable `direct-message` lease for the
  peer; each message still has its own bounded attempt lifetime.
  Explicit subscriptions remain available for exact resources, deadlines, and
  async results. No registration, absent, or unknown means no App Server
  input.
- `/goal` delegation and interactive task recognition are deferred.

Scoped capabilities replace roles: task owner for one task, resource holder
for one resource, integration lease for one main merge, and daemon operator for
one maintenance action.

## Board work and queued tasks

```sh
collab board show
collab board publish <id> --title <title> --description <scope> \
  --delivery-condition <result> --test-condition <check>
collab board claim <id> --expected-revision <observed-revision>
collab board update <id> --expected-revision <observed-revision> \
  --status blocked --next "<cause; release condition; next action>"
```

Publish additional IDs to queue subsequent work. Pending entries do not start
execution or occupy its slot. On completion or a blocker, inspect remaining
work and continue an authorized, nonconflicting task. Blocked and waiting work
keeps its owner and resources. Resuming it rechecks execution capacity.
Before idle and on the next activation, recheck blockers and unfinished duties.
These are agent actions; the board adds no periodic worker wake or automatic
execution. The [Collab Skill](skills/collab/SKILL.md#continue-the-queue-and-recheck-blockers)
defines each role's steps. Managed private queues are visible only to the child
and its parent; the public board shows parent responsibility for public work.

Long-horizon goals belong to the invited or claiming peer. That peer
orchestrates bounded execution and observation through independent
`codex exec --profile gcm` workers. Master and peer have the same orchestration
function at different goal levels: master coordinates the larger project goal,
and peer coordinates a delegated subgoal. Bounded execution and observation go
directly to subworkers regardless of peer idleness. `collab subagent dispatch`,
`collab subagent send`, and managed-subagent records are legacy compatibility
for diagnosis or migration, not a production assignment path.

## Stable names and session handoff

Names own project work; the reported agent sessionID is the current execution
carrier. Same-name session changes update all unclosed board assignments while
preserving task identity, status, evidence and resources. Closed history stays
unchanged. tmux session, pane and socket are transport placement only.

The old pane-default display-label model is retired. Read the current
[name and work ownership contract](../docs/design/collab-stable-name-work-ownership-20261020.md)
and the [Skill upgrade handoff](skills/collab/SKILL.md) before recovery or
migration. Those documents distinguish target design, source verification and
loaded runtime evidence; updating a Skill does not activate a daemon version.

Desktop agents use Collab CLI for registration and messaging; MCP tool calls
are optional. The identity anchor remains the agent sessionID. Desktop bridge
uses a separate normally managed transport. Its caller
context comes from executor metadata, and its recipient comes from the current
registered route. The daemon connects the bridge's local socket rather than
the Desktop tools pipe or an unrelated managed AppServer. See the
[Desktop bridge design and acceptance boundary](../docs/design/collab-desktop-bridge.md).
Source, installed MCP/daemon loading, native acceptance, actual wake, consumption
and reply evidence are reported separately.

## Install

Build once from the reviewed current source, install those exact binaries, then
refresh the embedded Skill:

```sh
scripts/install-global-collab.sh
```

The Cargo package baseline is `0.2.0`. Every official release compile uses
`scripts/build-collab.sh`, which increments the host-global
`~/.collab/build-version` counter under one lock before building. Release
builds therefore report `0.2.0001`, `0.2.0002`, and so on across worktrees.
Direct release builds fail with an instruction to use the official entry.
The installer performs one such build and installs those exact bytes; it does
not rebuild with a second version.

The canonical pair is `$CARGO_HOME/bin/collab` and
`$CARGO_HOME/bin/collab-mcp` (default `$HOME/.cargo/bin`). The sequence invokes
the exact newly installed binary to refresh the globally discovered Skill, so
an older PATH entry cannot write its embedded Skill. It does not remove legacy
copies, restart the global daemon, or touch `~/.collab/`, project-local
`.agent-collab/`, AppSDK state, business source, or evidence. An existing
daemon may keep running the old binary until an explicitly authorized
maintenance window.

## Start and identity

```sh
cd <project-main-tree>
collab context
```

The CLI observes available runtime facts. The daemon establishes or recovers
identity, verifies the native endpoint or tmux pane, persists the binding, and
restores the default notification lease. It returns the complete state snapshot.
If facts are missing, `requires_identity_update.required_fields` names them.
Supply only those real values once with `collab context --provide '<JSON>'`.
AppServer supplement keys are `session_id`, `thread_id`, `endpoint`, and
`namespace`; Desktop uses the separate `desktop_bridge` capability. See
[Desktop CLI registration](skills/collab/references/migration-daemon.md#desktop-cli-registration-and-messaging)
for the command shape, automatic discovery and first-enrollment limit. No worker
selector or separate migration command is needed.

The context bootstrap also merges the shared `collab-mcp` server into project
`.mcp.json` and writes the project CLI permissions Codex and Claude Code need
so `collab` can reach the App Server socket without a sandbox prompt. The
`collab` CLI is the ordinary agent action entry, whether MCP tools are listed
or not. Desktop still needs a live bridge for daemon-to-chat delivery.

For an ordinary long-horizon assignment, publish the task with delivery and
test conditions, then claim it or invite a suitable peer and respond with the
observed board revision. Collab master is not Codex root. Protocol:
`collab master status`, user-authorized `collab master promote --approval`, and
`collab master delegate` by the current master grant holder. Init and register
never create a master. Independent peers may decline a master collaboration
invite. A legacy managed-subagent record does not turn its holder into an
active production worker; master-only rules still require an explicitly
authorized master grant.

## Independent task lifecycle

```text
read latest main
→ declare private worktree/branch
→ register own task
→ sync latest main into own branch
→ implement/test
→ commit exact change set
→ sync latest main again
→ verify candidate against latest main
→ acquire short integration lease
→ merge exact commit to main
→ verify/push main
→ mark merged
→ close performs mandatory cleanup and persists a cleanup receipt
```

The ordinary peer assignment path uses the public board:

```text
collab board publish <id> --title <topic> <conditions>
→ collab board claim <id> --expected-revision <revision>
  or collab board invite <id> --to <peer> --expected-revision <revision>
→ the invited peer responds with the observed revision
→ task update/deliver/review/integrated/close
```

```sh
collab task register <id> \
  --feature <feature-id> \
  --worktree ./playground/<short-slug> \
  --branch codex/<branch> \
  --base-commit <sha> \
  --priority p2 \
  --next "implement and verify"

collab task update <id> --status verifying --next "run gates"
collab task update <id> --status reviewed --next "record delivery"
collab task deliver <id> --evidence "commit=<sha>; gates=pass" \
  --worktree ./playground/<short-slug>
collab task review <id> --accept --evidence "review gates=pass"
collab task integrated <id> --commit <main-sha> --evidence "main gates=pass"
collab task close <id>
```

Normal states are `working`, `blocked`, `waiting`, `verifying`, `reviewed`,
`delivered`, `accepted`, `rework`, `merged`, `closed`, and `cancelled`.
`waiting` must be entered through `collab task wait`; `delivered` through
`task deliver`; `accepted` through `task review --accept`; `merged` through
`task integrated`; and `closed` through `task close`. Review and integration
require the task owner or live master, and both persist their evidence. Direct
status mutation cannot bypass these gates for current lifecycle records. For
compatibility, an owner may move a persisted pre-review `accepted` task with
no lifecycle evidence directly to `merged`; this path records task state only
and cannot manufacture review or integration evidence.

Delivery is a local durable milestone. It sends no peer message. Every task
with a declared worktree carries a cleanup obligation. Close fails before
mutation unless the task is merged, its declared worktree is clean and inside
`./playground/`, and its branch is merged into current main. Close removes the
exact worktree/branch, verifies absence, persists a durable cleanup receipt,
and only then marks the task closed. A terminal task with a missing receipt or
an existing declared worktree fails audit; cancellation cannot bypass cleanup.

## Resource conflicts and waits

When a registration conflicts on `feature_id` or `worktree_path`, the Server:

1. persists the requested task as `blocked`;
2. returns structured `TASK_RESOURCE_CONFLICT` data, including the holder; and
3. leaves any peer notification to an explicit `sendmessage` operation.

The blocked owner may record a bounded wait:

```sh
collab task wait <task-id> --for <blocking-task-id>
```

Every wait stores waiter, blocking task, blocking owner as responsible actor,
reason, deadline, resume events, and `resource_owner_and_waiter_recheck`
escalation. Direct, two-peer, and transitive cycles fail closed. Missing owner,
missing deadline, unrelated resource, and delivered/terminal waits fail
closed. Timeout changes the waiter to explicit `blocked` and never releases a
claim. Holder close moves each waiter from `waiting` to `blocked` and clears
the obsolete wait edge. Notification occurs only for an exact active
resource-release/deadline subscription.

Manual P2P messages use `notify` and require a short subject plus a non-empty
durable body:

```sh
collab sendmessage --to <peer> --subject resource-busy "RESOURCE_OCCUPIED ..."
collab sendmessage --to <peer> --subject result-ready "The result is ready; query the mailbox."
```

Never type peer messages into a terminal. Without the recipient's active
`direct-message` subscription, the message remains mailbox-only. Registration
normally creates this subscription automatically; App Server receives the
short message id, abbreviated subject, safe one-line original body preview,
and submit as one queued operation. Notifications deliver only when an agent
is idle (waiting); if the agent is working, delivery defers without burning
wake attempts until the working->idle transition occurs. When the App Server
thread is dead, unowned, or its registered identity has changed, the
subscription enters its explicit unavailable state. If unacknowledged delivered notifications reach the limit
(default 3, configurable 1..=5), push knocks pause awaiting `collab ack <id>` or
`collab ack --all` to prevent backlog flooding and terminal pollution. When delivery
occurs with a larger backlog, batches are capped at 3 with inbox reminders. Worker
status, identity validity, and notification pressure can be inspected via
`collab worker status [id]` and `collab who`.

## Explicit notifications

```sh
collab notify methods
collab notify subscribe --event direct-message --ttl-seconds 600
collab notify subscribe --event resource-released --subject <task-id> --ttl-seconds 3600
collab notify subscribe --event deadline --subject <timer-id> --at-ms <epoch-ms> --ttl-seconds 3600
collab notify subscribe --event deadline --subject <timer-id> --every-ms 900000 --repeat-count 3 --ttl-seconds 3600
collab notify status
collab notify unsubscribe <subscription-id>
collab context
collab inbox
```

Subscriptions are owner-scoped, exact-event, and bounded by TTL. Each Agent
may hold at most three active subscriptions. A deadline uses either one or
more absolute `--at-ms` values, or a periodic `--every-ms` schedule with a
finite `--repeat-count` from 1 through 100. These modes are mutually exclusive.
The final delivery says it is the last reminder and instructs the Agent to
explicitly subscribe again; no automatic rearm exists. When a consumed
one-shot goal or deadline wake is followed by continued scheduling, the owner
must explicitly re-arm it before the patrol cycle ends. The default
`direct-message` lease accepts later peer messages until expiry; resource and
deadline event matching remains owner-scoped and finite.
App Server receives `COLLAB_NOTIFY <message-id> [<subject>]
<original-body-preview>` through the registered native thread. The Agent first
weighs the id and subject against current work. When it selects the notice, it runs
`collab msg <message-id>`, reads durable detail, and executes actionable
in-scope work; it must not stop at ACK or waiting. The first pending notice opens
a fixed 120-second window. At its end all eligible unsent notices for that peer
are combined into one bounded batch, including notices arriving later
in the window. Working Agents receive the batch without waiting for idle.
Attempts are reserved durably before the App Server queue; failure,
unknown/absent Agent, and
restart never replay an attempted batch. Details remain in the inbox.
Each recipient has at most one batch attempt per 120-second window. The daemon never creates periodic
`CONTINUE_TASK` messages.

## Existing-project migration

Never delete/rebuild `.agent-collab`, edit task/claim/journal JSON, clear the
mailbox, copy tokens, start a second daemon, or invent an owner. Use:

```text
collab migrate inspect
→ collab migrate plan
→ collab migrate apply          # admission freeze + deterministic snapshot
→ scripts/install-global-collab.sh
→ collab context                # daemon reconciles the current runtime identity
→ collab migrate verify         # verify and resume
```

Any authenticated peer may acquire the single migration transaction lease.
Another peer cannot replace an active plan/apply operator. Legacy role fields
and obsolete heartbeat/continuation fields are ignored during replay and
omitted from new state. Legacy `available` tasks or waits without a real blocker owner require
explicit operator resolution; the daemon never fabricates ownership. Malformed
journal lines fail startup, and a snapshot/count mismatch remains frozen.

The active lifecycle contract is
[docs/collab-v1-lifecycle.manifest.json](docs/collab-v1-lifecycle.manifest.json),
with source-bound adjacent edges in
[docs/mainline-call-map.json](docs/mainline-call-map.json).

## Daemon maintenance

The global daemon is managed by the user. Agents never start, stop or restart
it. Maintenance commands and credentials are outside agent help, MCP and Skill
surfaces. Ordinary context restoration updates the current identity, endpoint,
route and receive lease together. It never requires a maintenance window.

## Core invariants

- One Server writer; append-only journal; deterministic replay.
- App Server is the only registered transport; no control truth is inferred
  from terminal text.
- No fallback, silent strip, automatic ownership, automatic claim release, or
  success projection from failed wake/cleanup/migration.
- No uncommitted product edits in main as an intermediate test step.
- One task owner, one declared worktree, one resource holder, and one migration
  transaction operator at a time.
