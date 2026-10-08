---
name: collab
description: "已启用并注册 Collab 的 peer/master 通信、任务、claim、共享任务板与资源协作；明确要求注册时也用本 Skill。未注册独立完成，不寻找 Master；普通开发、worktree 与 goal 不自动启用 Collab。只跑 collab context 一条命令，由 CLI 自动观察运行事实、daemon 选择或恢复身份并返回 peers/master/scheduling/env 快照；缺事实时返回 requires_identity_update.required_fields，Agent 用 collab context --provide '<JSON>' 一次补充。高频: sendmessage, recv, task accept/update/deliver/review/close, collab board publish/invite/respond/update, collab dashboard, master 派单 collab subagent dispatch。review --accept 会登记 daemon pending merge, task integrated 才能 close (TASK_MERGE_PENDING)。"
---

# Collab

## 适用身份

只有本任务明确启用 Collab、当前执行者已注册且具有有效通信 route，才适用后文 master/peer/subworker 协议。未注册 Collab 就独立完成任务，不寻找、等待或服从 Master，不为普通任务自动注册、晋升或接管他人身份。用户明确要求注册时才执行初始化。

独立开发使用全局规定的外置独占 worktree。注意共享资源冲突，不改动、回收或覆盖他人的 worktree、文件、进程与 claim；能隔离就继续，不能隔离只报告受影响操作。没有 Collab 不阻断独立任务，也不要求建立一套协作生命周期。

共享任务板与 dashboard 同样只在已注册时可用：`collab board show` 需要已注册身份但不需要 daemon 生命周期动作；`collab dashboard` 只读，不注册 peer、不启动 daemon、不消费消息。

Durable identity, role, mailbox, task, and subscription truth lives in the
Collab daemon. Codex AppServer RPC is the preferred communication and thread
state transport. A tmux-only peer may be selected when it has no AppServer
candidate; once an AppServer binding is selected, tmux is never a message,
wake, presence, or status fallback. A verified tmux endpoint may also be stored
as a last-resort identity recovery anchor when both runtime IDs are unavailable.
Production projects use the globally installed Collab v1.

## Current-version baseline

Use only the current globally installed `collab` and `collab-mcp`. The
`Cargo.toml` version is the semantic source baseline; every official release
compile uses `scripts/build-collab.sh`, which increments the host-global
`~/.collab/build-version` counter under one lock. The installed binary's
`collab --version` reports `0.2.NNNN` as the runtime build version. Direct
release builds fail with an instruction to use the official entry. An upgrade
targets the current reviewed source and does not migrate, replay, or interpret
older local versions.

The canonical candidate delivery sequence is a single source, single
sink DAG. Every applicable node must finish before claiming delivery:

```text
latest_main_candidate
  -> targeted_tests + build/install
  -> if runtime/daemon is affected: collab down -> collab up (one maintenance window)
  -> installed binary + collab context + collab-mcp initialize live checks
  -> black-box live behavior replay + author debug complete
  -> applicable independent architecture review PASS
  -> commit/merge/push and cleanup
```

For ordinary peer bootstrap and identity recovery, the only entry remains
`collab context`, plus one factual `--provide` supplement only when required.
That command is not a daemon lifecycle owner. A Collab runtime
delivery that changes server, notification, identity, route, MCP, CLI, or daemon
behavior must cross the runtime lifecycle boundary in the same delivery unit
after installation, unless the owner explicitly records why no daemon change is
applicable.

The canonical install sequence from the candidate source is:

```sh
scripts/install-global-collab.sh
```

The installer performs one release build and installs those exact candidate
bytes; it does not run a second build with another auto-incremented version.
The canonical pair is `$CARGO_HOME/bin/collab` and
`$CARGO_HOME/bin/collab-mcp` (default `$HOME/.cargo/bin`). The sequence invokes
the exact newly installed binary to refresh the embedded Skill, so an older
PATH entry cannot write a stale Skill. The install does not remove business
source, Git history, `~/.collab/`, project-local `.agent-collab/`, AppSDK
state, run notes, or shared evidence.

Legacy user-local copies are not removed automatically by this sequence.
If an exact old copy must be retired, first prove it is Collab by running its
own `--version` (and for MCP, its `initialize` response), then remove only the
verified pair. A path that cannot prove that identity is a collision: preserve
it and report the exact path. Never delete `~/.local/bin/collab*` or
`~/.local/lib/collab/*` merely because the pathname matches.

Installing a new binary does not itself replace a running daemon. Do not run
`collab down` or `collab up` merely to refresh a peer identity. For a verified
runtime delivery, use one controlled maintenance window and preserve old/new
PID/socket, binary version/digest, identity, journal/mailbox, context, MCP, and
live replay evidence:

```sh
collab down
collab up
collab context
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"codex","version":"0.0.0"}}}\n' | collab-mcp
```

Never use a broad process kill or a second daemon. If `collab context` reports
a daemon PID that predates the maintenance window, the runtime node is
`INCOMPLETE`, not delivered.

Before changing the installed binary, inspect the current source version,
installed version, canonical paths, and daemon PID/socket:

```sh
cargo metadata --no-deps --format-version 1
collab --version
command -v collab
command -v collab-mcp
collab status --all
```

If the version or command path is stale after installation, fix PATH or refresh
the shell command cache (`rehash` in zsh, `hash -r` in bash), then verify that
`command -v collab` and `command -v collab-mcp` resolve to the exact
`$CARGO_HOME/bin` pair.
Do not hand-copy binaries, leave a second managed entry, or select an older
binary as a fallback.

## One lifecycle loop

Every master goal, bug report/fix, master-to-subagent assignment, and peer task
uses one authoritative `Trigger -> Work -> Gate -> State -> Stop` loop:

1. Discover current durable truth, ownership, dependencies, and evidence.
2. Persist durable dispatch intent before attempting notification.
3. Hand off one scoped assignment with delivery and test conditions.
4. Verify the real result; unknown, timeout, and failure remain explicit.
5. Persist the verified state, evidence, blocker, or failure.
6. Schedule the next eligible work, or stop at the loop's terminal condition.

No ACK, fallback, retry, snapshot, or notification may fabricate success or
replace a missing gate. Bug reports and fixes enter the AppSDK or git-bug
backlog with investigation evidence. P0 is highest priority and blocks the
affected project.

## Recovery, Failure, Reset, Regression

### 1. Recovery

Start with the one-step bootstrap. `collab context` is the single automatic
state entry. The CLI resolves the canonical project root, creates a missing
baseline, starts the daemon unless an explicit `DOWN` marker exists, observes
the available runtime facts, and sends a typed identity request to the daemon.
The daemon owns identity selection, creation, recovery, update, credential and
binding persistence, route publication, and default direct-message lease
restoration. It returns either a registered snapshot or an explicit
missing-facts result. The active contract is
[`docs/design/collab-identity-minimal-interaction.md`](../../../docs/design/collab-identity-minimal-interaction.md)
with recovery semantics in
[`docs/design/collab-anchor-restore-model.md`](../../../docs/design/collab-anchor-restore-model.md).

```sh
collab context
```

The daemon recovers identity by anchor. Each transport has one designed anchor:
the tmux owned pane, the AppServer session/thread, or the dsh session. An anchor
that matches a persisted identity restores it directly, with no probe, no
runtime state and no generation requirement. An anchor with no match drafts a
new identity. Matching is never guessed: an ambiguous anchor or a cross-project
match fails explicitly.

You observe nothing by hand. `DSH_SESSION_ID`, `CODEX_SESSION_ID`,
`CODEX_THREAD_ID` and the tmux pane are all read by the CLI. A gateway control
socket is not something to look for: the dsh anchor is `DSH_SESSION_ID`, which
is observable without a gateway.

That anchor settles *who* you are, not *how* you register. A dsh transport
needs the gateway's `endpoint`, `runtime_id` and `agent_id`, which the CLI
cannot observe. With no gateway, a dsh caller — new or already persisted —
fails at `TRANSPORT_NONE`. That is a gateway failure, not a missing identity,
and it never retires the peer.

When the snapshot contains `requires_identity_update`, read
`required_fields`, `reason`, and `exact_error`. Only the AppServer path returns
fields here. Supply only the requested facts from their real source once:

```sh
collab context --provide '{"session_id":"...","thread_id":"...","endpoint":"...","namespace":"..."}'
```

After successful registration, the daemon reuses missing facts from the
committed identity when the current observed anchor uniquely identifies it.
Later calls need no repeated supplement. A caller with no current anchor must
provide its own facts; it cannot inherit another caller's identity.

The supplement accepts only the missing scalar facts `session_id`, `thread_id`,
`endpoint`, and `namespace`. It rejects any other key. An agent never guesses,
selects, or hunts a worker, and never supplies `worker_id`, approval, token,
generation, binding, or project scope. Invalid or unsupported input fails
explicitly. The returned snapshot is the complete state read: role, operations,
peers, master, scheduling, tasks, binding, and filtered environment. `binding`
comes from the daemon ledger when the caller has a unique binding.

If `collab context` fails explicitly, preserve the original error and affected
scope. Do not retry automatically, edit route/identity files, copy tokens,
start another daemon, or invent a fallback. Daemon restart remains a separate
controlled maintenance action after a verified runtime delivery. Ordinary
identity recovery uses `collab context` only.

Master authority is separate from identity. The authority owner is the current
typed grant in the exact project and app scope, with only two states: empty (no
current grant) and assigned (one current holder). When the user explicitly
approves this session for this exact project, run one command with their words
as the approval:

```sh
collab master promote --approval '<why the user approved this>'
```

An approved promote atomically replaces any recorded holder and is not gated on
the incumbent's or the candidate's transport liveness. A tmux pane and an
AppServer oracle are addresses, not credentials: neither being reachable is
allowed to strand authority. Explicit approval is the whole authority for the
transition. To remove the scoped authority instead, an authenticated peer with
explicit approval runs `collab master clear --approval '<why the user approved
this>'`; clear removes only the scoped grant and never deletes tasks, messages,
peers, or bindings. Context never auto-promotes and never infers authority.

### 2. Failure

Treat each claim separately:

```text
durable=true -> mailbox journal accepted
notification=accepted -> selected transport accepted the operation; it is not consumption
recv receipt -> peer consumed the message
task close receipt -> lifecycle ended
```

An error, timeout, `subscribed-not-sent`, `thread-lost`, `identity-mismatch`,
`unknown`, or absent Agent is not success. Preserve the exact error and
durable IDs; do not retry automatically, ACK for another identity, or mark a
task delivered/closed without its required evidence. A worker reports the
root cause and proposed fix to the current master. The master takes ownership by
fixing, re-dispatching, or force-closing with an auditable reason.

Transport observations (`unknown`, `cold`, `missing`) are separate from
authority. They never create, replace, or clear the current master grant. Do
not promote, clear, or regrant the master merely because a transport probe is
unknown; the holder keeps control, and the communication error stays explicit
and independent.

### 3. Reset

Use reset only when the operator explicitly authorizes discarding the named
control-plane state. Reset is offline, transactional, and requires an explicit
level; it is not migration and it does not preserve the state it retires:

```text
collab down/up -> controlled daemon restart; journal/mailbox survive
collab migrate -> authenticated migration and identity rebind
collab reset --project -> retire the project control plane and rebuild
collab reset --host    -> rebuild the host control plane
```

Every level takes the same host writer lock as the daemon, requires the daemon
to be down, stages the bytes it will remove under `~/.collab/archives/`, verifies
the archive, and appends one record to `~/.collab/reset.jsonl` that carries the
approval text. A level is required: a run with no level, or with two, fails with
`RESET_LEVEL_REQUIRED`, and a flag the selected level does not use fails with
`RESET_LEVEL_FLAG_MISMATCH` rather than being ignored. Reset is idempotent, and
it never imports old PASS or delivery claims.

`--project` is the historical level. It retires the current project's
`.agent-collab/` and `.agent-collab-v2/` bytes, rebuilds the current empty
scaffold, and refuses when that project root holds the live host index
(`RESET_PROJECT_HOLDS_HOST_INDEX`). It takes no path argument; run it from the
project root.

`--host` retires the host control plane under `~/.collab/` (`routes.jsonl`,
`journal.jsonl`, `events.jsonl`, `log.txt`, `identities/`, `projects/`) and the
resident project's `.agent-collab/server/` journal, events, and log. It keeps
`~/.collab/reset.jsonl` and `~/.collab/archives/`, because they are the audit
trail, and it keeps the external service descriptor and the daemon's own socket
and lock files. The project business payload under
`.agent-collab/{mailbox,messages,handoff,merge-queue,runs,mailboxes}` belongs to
`--project`, not here. `~/.collab/runs/` survives unless `--include-runs` is
given. No baseline is rebuilt: the next `collab up` starts from empty. `--host`
requires `--storage-root <project root>`, the root that owns the live host
index, and fails with `RESET_STORAGE_ROOT_REQUIRED` when it is absent.

The routes level was removed: one pane owns one route binding host-wide
(`docs/design/collab-pane-route-ownership-20261006.md`). Every level records
`delivery_verified: false`. Reset alone is not delivery, review, install,
restart, or live-communication evidence.

`.appsdk/` and `.appsdk-control/` are AppSDK-owned and are not removed by
`collab reset`. The host-wide runtime truth remains `~/.collab/`
(`server.sock`, `events.jsonl`, `log.txt`, and route state); project-local
`.agent-collab/` is reducer input and local durable data, not the global truth.
For a new project or an explicitly authorized clean epoch, remove old
project-local governance only through the owner's canonical reset/migration
command. Never manually remove `.agent-collab/server/journal.jsonl`, mailbox
files, identity tokens, task records, or bindings to make status look clean.
Never start a second daemon or use broad process kills. `collab ack` remains a
compatibility operation; it is not a substitute for task close or identity
recovery.

An AppSDK `reset-governance` or `appsdk init --fresh --discard-legacy` is not
Collab reset/migration and must not remove `.agent-collab/`. AppSDK's reset owner
explicitly treats `.agent-collab/` as a reserved root. If the project also
needs to move or retire Collab state, follow
[Migration and Daemon Maintenance](references/migration-daemon.md); the two
operations have separate transactions and separate completion evidence.

### 4. Regression recognition

After a fix, verify the same user path again and classify the first divergence:

- `send` durable but no transport acceptance: inspect the selected transport,
  subscription, ownership, Agent state, and daemon log.
- tmux input submission appears but no worker result: inspect the durable
  mailbox receipt and peer state; do not call submission a reply.
- `recv` returns messages: the read is consumed atomically; no follow-up ACK is
  required. `msg`, `inbox`, and `context` remain read-only.
- task remains open: inspect owner identity, master responsibility, cleanup
  receipt, and notification supersession.

Record the tested source commit, binary digest, daemon PID/socket, exact
commands, and live replay result in the bug system. A test pass without
latest-main merge, installed-binary verification, or applicable live replay
does not close the bug.

## Automatic multi-worker collaboration

Keep Collab enabled. At multi-worker startup, run `collab context` once
in the inherited live peer environment. This lets the daemon establish or
recover the peer and the default finite direct-message subscription.
Registration returns `role_brief`. Read it as the active operating contract.
It is the registration-time projection; `collab context` returns the current
brief, and promotion or delegation returns the replacement brief. Do not
maintain a separate role prompt:

- `master`: dispatch and allocate resources, keep workers loaded, own blockers,
  and drive verify/merge/cleanup/close. Implementation is not the primary job.
- `worker`: complete the independently owned task end to end, evaluate master
  collaboration requests against current ownership/capacity, and explicitly
  accept or negotiate rather than ignore.
- `managed-subagent`: execute the assigned scoped task, obey master/parent for
  that assignment, and return root-cause/evidence rather than build a global
  schedule.

The default identity is peer. Master authority is explicit and
user-authorized: an approved `master promote` replaces the recorded holder,
and delegation is accepted only from the current master grant holder and
records that handoff. Registration, a process, inferred `/goal`, or
`role_brief` never silently creates master authority.

On trouble, every non-master investigates first and reports the current
master: root cause, attempted actions, proposed fix, and exact decision needed.
A role change via `master promote` or `master delegate` returns the new master brief;
the old worker brief no longer governs that peer.
Once task scope and the independent worktree are known, automatically follow
[task/worktree registration](references/task-worktree-lifecycle.md): bind the
task, feature/resource and owned file scope before concurrent product edits.
Use [resource coordination](references/resource-waits.md) for overlaps; never
share a worktree or overwrite another peer's files.

Registration, necessary coordination and subscriptions require no repeated
user confirmation within an authorized multi-worker task. Send only messages
authorized by the user's collaboration request; no unrelated external notices.
Do not require a serial merge queue for communication or read-only work.

If initialization fails, report collaboration unavailable and preserve its
error. Shared writes and dependent coordination wait for reliable ownership;
independent isolated work and AppSDK quality checks can continue. No invented
peer, local substitute claim or fake successful registration.

## Send an ordinary message

Run exactly one command:

```sh
collab sendmessage --to <peer> --subject <short-topic> "<original message>"
```

`--to`, a non-empty short `--subject`, and the original body are required.

Do not first run `notify methods`, `notify subscribe --help`, or a separate
identity probe, or choose `mailbox-only`. There is no separate mailbox-only
send mode.
`sendmessage` always commits the full subject/body to the durable mailbox.
With a matching live subscription, the first pending message opens a fixed
120-second window by default. `~/.appsdk/config.toml` can select immediate or
batched delivery globally or per project; `appsdk config` shows effective
policy. All eligible unsent messages for that recipient are combined
into the selected transport (up to 3 previews per notification, with overflow
retained in the inbox). On an AppServer binding, the daemon uses
`turn/start`, `turn/steer`, or `thread/queue/add`; on a tmux-only binding it
submits a bounded wake to that pane. Neither transport acceptance proves that
the peer consumed the durable message, and neither binding falls back to the
other transport after selection.
Explicit `collab sendmessage` is
immediate and follows the explicit-message adapter gate, including while the
recipient is working.
If delivered-but-unconsumed notifications reach the throttle threshold (default
3), further push knocks pause until `collab recv` consumes them, preventing
terminal pollution and storms. Each batch has one attempt; the default window
is 120 seconds. Policy changes require controlled daemon restart, not task
reset.

Explicit `collab sendmessage` is immediate. Idle, progress, delivery, bug, and
worker-idle notices are auto-merged by the daemon in the 120-second batch
window; they are not repeated as heartbeat storms.

Do not retry a failed send automatically. Return its exact error and durable
status. Never call a transport command directly; the server owns transport
selection and sends only through the selected adapter.

## Public task board and read-only dashboard

The public task board is the shared master/peer surface. It carries only public
task identity, lifecycle status, owner, and the typed delivery/test conditions.
It never exposes worker tokens, runtime endpoints, role briefs, managed-child
identity, private assignments, or raw journal events. A published task does not
start execution; an invitation does not transfer ownership.

Read the board without consuming messages or changing lifecycle:

```sh
collab board show
```

The current master grant holder publishes a pending task with its contract,
then invites one live idle ordinary peer:

```sh
collab board publish <task-id> --title <t> --description <d> \
  --delivery-condition <d> --test-condition <t> [--priority p0|p1|p2|p3|p4]
collab board invite <task-id> --to <peer> --expected-revision <observed>
```

Invite and respond are revision-checked. `invite` fails with
`BOARD_STALE_REVISION` when the observed revision is old, and with
`BOARD_PEER_BUSY` when the target already owns an unfinished task or another
invitation. Take the revision from `collab board show`, never from a message
body.

The invited peer accepts or declines in its own identity. Accept rechecks the
peer's other responsibilities and its live binding before ownership moves:

```sh
collab board respond <task-id> --accept --expected-revision <observed>
collab board respond <task-id> --decline --expected-revision <observed> --reason "<text>"
```

Decline requires a nonempty reason and leaves publisher ownership unchanged. The
owner updates progress on its own task with a current revision:

```sh
collab board update <task-id> --expected-revision <observed> --status <s> [--next <step>]
```

The master withdraws an unaccepted invitation with its observed revision and a
reason; withdrawal never reclaims peer-owned execution:

```sh
collab board withdraw <task-id> --expected-revision <observed> --reason "<text>"
```

Ordinary peers do not receive scheduler dispatch. `collab subagent dispatch`
targets the private managed path only; a public assignment uses the board
invitation above. If a peer or its peer node refuses an invitation, that refusal
is explicit and there is no silent fallback to a private child.

`collab dashboard` opens a read-only loopback Web observer for the same public
board. It requires a registered identity, does not bootstrap or restart the
daemon, and never consumes messages:

```sh
collab dashboard [--port <loopback-port>]
```

It prints one JSON line and then serves until stopped:

```json
{"url":"http://127.0.0.1:<port>/#<capability>","read_only":true,"refresh_seconds":2}
```

Open that printed URL in a browser on the same host. The capability is a fresh
per-run read credential carried only in the URL fragment: the page keeps it in
memory, sends it as a bearer header, and never puts it in a query string or a
log. It grants no actor identity and no write authority. The page re-reads the
board projection every 2 seconds; it is a polling view, not an event stream.

Any registered identity on that host may start one, so a master or an ordinary
peer can expose the project board for the human controller. It is an observer,
not a control surface: it cannot publish, invite, respond, update, withdraw or
close anything.

The observer rejects a request whose `Host` is not its own bound authority with
`DASHBOARD_HOST_REJECTED` (403) and a request without the exact capability with
`DASHBOARD_CAPABILITY_REQUIRED` (401). While the daemon is unreachable the
board endpoint answers 503 with the exact daemon error instead of an empty
board. Static assets carry no state and no token.

The public board is for the master and ordinary peers only. A managed subagent
actor is refused with `BOARD_PRIVATE_ACTOR`, managed children never appear in
the board projection, and `collab subagent dispatch` stays on the private
managed path.

An owner edits the descriptive part of its own independent task without
changing lifecycle. Every flag is required and the revision is checked:

```sh
collab board describe <task-id> --expected-revision <observed> --title <t> \
  --description <d> --delivery-condition <d> --test-condition <t>
```

`collab-mcp` exposes the same operations as `collab_board_show`,
`collab_board_publish`, `collab_board_invite`, `collab_board_respond`,
`collab_board_withdraw`, `collab_board_update`, `collab_board_describe`, and
`collab_task_decline`. Each takes `expected_revision` as an integer of at least
1; `collab_task_decline` takes `legacy_assignment: true` only for an unstarted
legacy assignment.

When any board command fails, the CLI preserves the server's full typed
response, including durable IDs and `repair_required`. Read the `collab
response:` line before retrying: a durable outcome is not a retryable failure.

## Common command card

Managed subagent thread creation is unsupported in the production path and
fails explicitly. Registered peer status and thread state use the owning
AppServer RPC. Use registered peers for concurrent work. Existing subagent
records may be inspected with `status`, `send <id> --subject <topic> "<task>"`,
and explicit `close <id>`.
`collab-mcp` is the shared Collab MCP for every agent. Use `collab_*`
tools when this session lists them. The `collab` CLI is also valid.
If MCP is missing, unsupported, aborted, or unknown, run the same
actions with the CLI in the inherited project cwd:
`collab context`, `collab recv`, `collab ack <id>` / `collab ack --all`,
`collab msg <id>`,
`collab inbox`, `collab worker status [id]`, `collab subagent ready|working <id>`,
`collab sendmessage --to <parent> --subject <topic> "<body>"`.
The CLI is a complete protocol path. Missing MCP is not a blocker and
does not justify skipping receive or waiting. `collab context` is the only
agent bootstrap entry; do not run a separate identity, route or archive probe.
A conflicting claim on your tmux pane (a stale claim, or one from another
worker or project) is resolved by the daemon inside that same one call, so you
never adjudicate a pane conflict and never need a second command for it.
`collab init` is not the default agent entry, but it drives the same daemon
identity gate and remains the entry for existing AppSDK init consumers.
Child results go to the parent with
`collab sendmessage`, not the parent-only `subagent send` action.
No ACK loops, automatic respawn or redispatch.

The current master grant holder is the sole scheduler assignment owner.
Dispatch through the durable scheduler path with a stable request ID:

```sh
collab subagent dispatch --request-id <id> --subject <topic> "<assignment>" \
  [--feature-id <feature>] [--worktree-path <path>] [--branch <branch>] \
  [--base-commit <sha>] [--priority p0|p1|p2|p3|p4] [--next-step "<step>"]
```

Flag and body rules:

- `--request-id` is required. ASCII letters/digits, `-`, or `_`, at most 80
  bytes. Stable and idempotent: after an audit interruption, retrying the same
  ID recovers the existing reservation and never creates a second task or
  message.
- `--subject` is required and non-empty.
- The body is required and must contain Goal, Scope (allowed/forbidden paths),
  Delivery iff, Tests (commands + expected + evidence path), Deliverables,
  Forbidden, and Flow. Ordinary peers receive only this body.
- `--feature-id` is optional: stable feature slug.
- `--worktree-path` is optional. When `[worktree].base` is configured it must
  be the rendered configured base path, for example
  `<base>/<project-key>/<short-slug>`; otherwise legacy
  `<project-main>/playground/<short-slug>` records remain valid while the
  transition is active. The leaf is at most 32 ASCII
  letters/digits/`.`/`-`/`_` and `..` is forbidden. The persisted field is
  `worktree`, not `worktree_path`.
- `--branch` is optional; prefer `codex/<short-slug>`.
- `--base-commit` is optional; prefer the current `origin/main` SHA.
- `--priority` defaults to `p2`; legal values are `p0|p1|p2|p3|p4`.
- `--next-step` is optional and tells the peer the first concrete action.

Only the current master grant holder may dispatch; the holder's control does
not depend on its own transport liveness. The scheduler selects an eligible
peer automatically: registered, non-requester, non-managed-subagent, presence
present, and no active task. A peer holds only one active task. If no eligible
peer is available the exact error is:

```text
MANAGED_SUBAGENT_UNSUPPORTED: no live registered tmux peer is available for dispatch
```

Do not create a fake peer or mark an ordinary peer as managed to bypass that
error.

Successful dispatch response fields:

- `request_id`, `message_id`, `task_id`, `target`, `status: assigned`.
- `admission.{request_id,message_id,task_id,worker_id,decision,status,reason}`:
  scheduler admission audit.
- `notification`: `sent` (transport accepted submission), `subscribed-not-sent`
  (subscription exists but notification was not accepted), or
  `mailbox-only-no-subscription` (no subscription; durable mailbox only).

`notification: sent` proves the sending side accepted the operation, not that
the peer consumed it. Check consumption on disk:

- `collab msg <message-id>`: `consumed_by_recv` false means not consumed.
- `collab task status <task-id>`: read `status`, `worktree`, `branch`,
  `base_commit`.
- A normal peer must run `collab task accept <task-id>` to change
  `assigned -> working`; `collab task update --status working` is rejected for
  an assigned task.

The scheduler reserves one message/task pair for an eligible peer, binds the
peer's active direct-message lease, records the admission audit, persists its
succeeded or failed status, and only then attempts one bounded notification on
success. An ordinary assigned peer must authenticate
with its own worker token and run `collab task accept <task-id>`; that command
atomically records `assigned -> working` and is the only receive entry for this
assignment. `collab task update --status working` is rejected for an assigned
task. A managed child accepts through `collab subagent working <id>`. Reusing a
request ID after an audit interruption recovers the existing reservation and
must never create a second task or message. The legacy `collab task dispatch`
and `collab task claim` commands remain deprecated and fail explicitly.

## Dispatch template

Copy and fill this template for a normal scoped assignment:

```text
Goal:
  Make <feature> work in the project.

Scope:
  Allowed: <paths/modules you may touch>
  Forbidden: <shared/control paths; daemon lifecycle; ~/.collab and
  .agent-collab state; routes, journal, mailbox, tokens>

Delivery iff:
  Complete only when <observable behavior> passes <specific checks>.
  Non-goals: <things not required; unrelated refactors>

Tests:
  Commands: <exact commands>
  Expected: <exact pass output/exit code>
  Evidence path: <file/JSON/log to attach in delivery>

Deliverables:
  <commit SHA>, <diff files>, <test output>, <real entry replay>

Forbidden:
  Do not merge/push, restart the shared daemon, install a global binary,
  edit control state, or touch files outside Scope.

Flow:
  Resolve root with collab context.
  Create/verify the assigned worktree <--worktree-path> on branch
  <--branch> at <--base-commit>.
  Implement scoped change, run tests, commit.
  Deliver evidence to master; report root cause + proposed fix if blocked.
```

Dispatch example:

```sh
collab subagent dispatch \
  --request-id fix-notify-lost-recv \
  --subject "recover lost recv delivery" \
  --feature-id notify-recovery \
  --worktree-path /path/to/appsdk/playground/notify-lost-recv \
  --branch codex/notify-lost-recv \
  --base-commit <origin/main sha> \
  --priority p1 \
  --next-step "collab context; reproduce lost recv; patch and test" \
  "Goal: ...
  Scope: ...
  Delivery iff: ...
  Tests: ...
  Deliverables: ...
  Forbidden: ...
  Flow: ..."
```

Consume notifications promptly with `collab recv`. A successful receive delivers
and acknowledges the batch atomically. After 3 delivered-but-unconsumed
notifications, push knocks pause automatically to prevent notification storms
and prompt pollution; `collab inbox` is read-only and does not resume delivery.
Use explicit `collab ack` only for legacy clients or recovery of an already
delivered message. Inspect peer/worker health, identity validity, and throttle
status via the `collab context` snapshot. The human diagnostic
`collab worker status [id]` and `collab who` print the same fields when an
operator audits them; neither is part of the default agent flow.

Worker wake model: only master has long-horizon wake; workers are not
long-horizon wake targets and are not automatically woken from idle. A worker
acts on an explicit dispatch, a bounded direct-message lease, or its own open
task state;
it does not need periodic activation to make progress. Unknown/absent produces
no transport input. On each `working` -> `idle` transition, a worker sends one
idempotent worker-idle fact to the current master and then stops; it does not keep
knocking. Idle, progress, delivery, bug, and worker-idle notices are
auto-merged; explicit `collab sendmessage` remains immediate. Master idle
reminders are level-triggered: within the same master idle episode, each
reminder attempt consumes the shared episode-local budget, up to three
attempts. An observed change to `working` ends that episode; after three
consecutive attempts without an observed `working` change, the episode stops.
Worker-idle facts do not count toward this budget, and there is no
scheduling-turn counter or automatic rearm.

Managed subagents do not get child-targeted periodic liveness ACK loops. Their
state is persisted by the daemon; a `working` -> `idle` transition contributes
one durable `subagent-status` fact to the current master. The master, not the
child, owns the outcome and decides whether to re-dispatch, force-close, or
leave the child idle. A subagent with an unfinished task is not repeatedly
woken just because its task is not closed. `subagent status` reports durable
task state and native thread state. Managed subagent creation remains
unsupported; commands fail explicitly. Codex thread history is read only from
the owning AppServer when supported and is never fabricated. A tmux pane is a
last-resort identity anchor, not a status observation or snapshot receipt.

## Cross-project master communication

Collab communication across projects is master-only and explicit. Only the
current master grant holder may send to the current master grant holder of
another initialized project; non-master peers and managed subagents are
rejected before any message is persisted:

```sh
collab master send --project /abs/path/to/target --to <target-master> \
  --subject <short-topic> "<original message>"
```

`--project` must be the exact target project root with `.agent-collab`, and
`--to` must be that project's current master grant holder, whose target binding
and address must be available. The target daemon also verifies the sender's
current typed grant (holder, scope, approval) before accepting the message.

Task liveness is an obligation, not an ACK ceremony. A worker owns its assigned
tasks and drives them to verified cleanup/close during its working cycle. This
is a task-bound inspect obligation, not a transport activation schedule: it
does not wake idle workers and does not generate worker transport input. If an actionable
task is open, continue it; if it is blocked, find a concrete solution first,
then report it to the current master in the same activation. Do not leave a task
at `assigned`, `working`, `blocked`, `waiting`, `delivered`, or
`cleanup_pending` merely because the last direct message was acknowledged.
After delivery or merge, perform the real cleanup and close the task; a
reminder does not create a second task or a duplicate dispatch.

Escalation routing is explicit:

- A managed subagent and an ordinary worker both report blockers to the current
  Collab master immediately. Do not wait for the next liveness cycle or for
  master to invent the fix. First find a concrete solution (root cause,
  proposed change, authorization needed); escalate that, not a symptom.
  Lazy thinking is forbidden: do not dump "I'm blocked" and idle. If the
  assignment's delivery or test conditions are ambiguous, do not guess;
  propose the missing conditions and send them to master. A subagent must
  also copy its parent when parent is not the master, and may not decline a
  master collaboration request. Independent peers may temporarily decline a
  master collaboration invite to protect their own current task. If no current
  master exists, report to the collaborator that initiated the task. Include
  the task ID, exact blocker, proposed solution, attempted actions, and
  requested decision.
- Master authority is the current typed grant in the exact project and app
  scope. Only two authority states exist: empty (no current grant) and
  assigned (one current holder). When the user explicitly approves this peer
  for this exact project, record it with `collab master promote --approval
  "<user text>"`; the approved promote atomically replaces any recorded holder
  and is independent of the incumbent's or the candidate's liveness. Only the
  current master grant holder may `collab master delegate <peer>`. An
  authenticated peer with explicit approval may `collab master clear
  --approval "<user text>"` to remove the scoped grant; the caller does not
  need to be the master first. An internal init adapter result alone never
  proves master ownership, and AppSDK init is not an implicit authority reset.
  Read the current holder, its scope, and its approval from the `collab
  context` snapshot and `collab master status`; agent liveness is a separate
  transport fact and does not change authority. A missing worktree-local
  `.agent-collab/`, a failed `collab context`, a token mismatch, or a missing
  `who.master` field never authorizes a mutation; promote, clear, and delegate
  still require an authenticated current binding. Codex root is not Collab
  master. Operators may use `collab master status` read-only for audit.
- If a blocker or wait cannot be executed locally after a real solution is
  found, report that solution to the current master immediately instead of
  silently waiting. Keep the durable wait/task state, continue any
  independent work, and re-escalate on the next direct master communication or
  when the situation changes; do not wait for a periodic worker wake.

## Master owns the outcome, not the excuse

Collab master is accountable for the final result of every assigned task
in the project. Once master accepts a dispatch, the master -- not the
worker -- is the escalation target, and the master cannot hide behind the
worker's blocker. Concretely:

- The current master must close any task that cannot otherwise be closed,
  including stuck or merged-but-unclean tasks, with `collab task close
  <id> --force --reason "<text>"`. The reason is recorded in the cleanup
  receipt so the manual close is auditable; notification obligations for that
  task owner are superseded.
- When a worker reports a blocker the master must take over ownership of
  the resolution: re-dispatch, close manually, or revise the assignment
  conditions. Master is not allowed to send an "I'm waiting on you"
  reply, mark the task blocked, and idle. If master cannot unblock the worker
  promptly, master force-closes the task with a reason so the loop stops and
  the worker's identity stays clean.
- When no master is assigned, the task owner may force-close its own task with
  `collab task close <id> --force --reason "<text>"`. If the owner's registered
  transport identity is missing and no master is assigned, a registered peer
  may close that orphaned task with the same command. An unreachable or unknown
  master remains assigned and does not permit these exceptions. These are the
  only allowed force-close exceptions;
  the reason and cleanup receipt are mandatory so the daemon can show who
  closed what and why.
- Master may not delegate its accountability by passing the task back to
  the worker and waiting. Master either solves, re-dispatches, or force-
  closes. Doing nothing on a stuck task is a master failure, not a
  worker failure.


## Master splits and assigns

The human remains the only final authority for goals, money, irreversible
risk, and version promotion. Collab master is the user-approved project
dispatcher, not the human 主脑 and not Codex root. Master compiles
the goal into a task graph, then assigns; it does not take another peer's
task or worktree.

**Master is an architect and dispatcher, never an everyday code-author.**
Master authoring business diffs is an anti-pattern and a failure of division
of labor. Master's scarce capacity belongs to task graph compilation, strict
dependency boundaries, unblocking workers, and driving overall throughput.
Master operates under two prime directives:
1. **Exemplary Task Decomposition**: Slice goals along clear dependencies and
   non-overlapping file ownership. Every dispatched task must be closed-loop
   by design: define explicit done-iff (DoD), artifacts, forbidden edits, exact
   test commands, expected results, and evidence location. Every assignment must
   anticipate failure and define an exception resolution path: **fallbacks,
   silent downgrades, or masking errors are strictly prohibited**. A blocked
   worker must produce root-cause evidence and proposed fixes for master
   arbitration; master must actively close the lifecycle rather than patch
   output symptoms.
2. **Worker Capacity Saturation**: Master must keep the entire worker fleet
   fully saturated without idle time or serial bottlenecks. Saturate every
   live present ordinary peer first, then schedule managed subagents within
   the configured `subagent.max_concurrent` cap. Peers do not consume that
   cap. Before ending each scheduling turn, inspect every live peer and
   managed subagent: if any eligible worker is idle and an authorized P0/P1
   task is ready, dispatch the next non-overlapping assignment immediately.
   Delivery, merge, review, task close, and cleanup are lifecycle steps, never
   reasons to leave capacity idle.

**Sovereignty and Backlog Priority**:
- **No autonomous technical debt refactoring**: When assigned tasks complete
  and the fleet becomes idle, Master is strictly forbidden from autonomously
  launching long-range technical debt refactors, speculative architectural
  rewrites, or unapproved work. Master must formulate a structured proposal
  for the human user and pause. The human is the ultimate decision-maker;
  unbounded autonomous runs risk destabilizing user intent.
- **Autonomous Bug Tracking Backlog Resolution**: If pre-existing issues or
  requirements are logged in the bug system (`appsdk bug list --status open`),
  these represent authorized project work. Master autonomously pulls and
  dispatches open bugs in strict priority order (P0 > P1 > P2) to keep
  worker capacity saturated before suggesting closure.

**Worker->Master idle fact and master long-horizon wake**:
When a worker transitions from `working` to `idle`, it emits one idempotent
worker-idle fact to the current master. The master has long-horizon wake; the
worker does not. On an idle fact, master must:
1. Check the active task graph for unblocked downstream tasks and dispatch;
2. If the main graph is clear, pull the highest-priority open issue from the
   bug backlog (`appsdk bug list --status open`), with P0 first; P0 blocks the
   affected project;
3. If all tasks and bugs are closed, report completion and propose next
   steps to the user.

Do not stop after consuming a report or closing a task while another live,
eligible peer has no assignment. Saturate live peers first, then use managed
subagents only within the configured cap. Dispatch the next ready
non-overlapping P0/P1 task before the scheduling turn ends. A fleet with idle
capacity is not a completed scheduling cycle.

Master idle reminders are level-triggered. Within the same master idle episode,
each reminder attempt consumes the shared episode-local budget, up to three
attempts. An observed change to `working` ends that episode; after three
consecutive attempts without an observed `working` change, the episode stops.
Worker-idle facts do not count toward this budget, and there is no
scheduling-turn counter.
Master must not expect workers to be woken periodically.

**Unacknowledged Workers & Pane Diagnostic Closure**:
If a worker fails to acknowledge notifications or remains unresponsive across
repeated dispatches, never blindly loop sends or expect the model to self-correct.
Execute diagnostic closure immediately:
```sh
collab worker status <id>
```
Use durable Collab state and the selected binding's status source to choose the
next action. For an AppServer binding, query native thread state. A tmux pane is
address proof only and never proves the agent is alive or `Present`; its agent
liveness stays unknown. Transport status cannot establish message consumption
without the receive receipt. A tmux screen cannot establish task completion,
peer consumption, or Codex turn state. Do not infer those facts or restart a
shared runtime from a pane probe.

Master keeps architecture, dispatch, integration, critical repair, and
final acceptance. Use registered peers and `collab sendmessage` for
assignments. Managed Codex subagent thread creation is unsupported and fails
explicitly. Give each peer its own worktree and file
scope. Independent peers may decline an invite to protect their current task.
Wait for evidence summaries, then integrate. Chat tone is not completion.

Delivery and review are not lifecycle endpoints. `task review --accept`
registers a daemon-owned pending merge and notifies the current master; the
obligation is durable, appears in `collab context`, `collab status --all`
(`pending_merges`), `appsdk longhorizon show` (待合并), and master idle wake
text, and `task close` fails with `TASK_MERGE_PENDING` until the master records
`collab task integrated` (or, if the peer performed the verified merge itself under the conditions in the next section, that same recording step still applies). Never rely on remembering a merge from a message.
After a delivered candidate, either the peer merges and pushes the verified candidate under the next section’s conditions, or the current master drives review, integration,
cleanup, task close, and then the next ready assignment. Do not leave a peer
idle merely because its last task returned `delivered`, `merged`, or a review
verdict. Reuse the same live peer
or a fresh managed subagent for the next non-overlapping P0/P1 assignment
whenever capacity exists. Closing or force-closing a stale task is a scheduling
decision that must preserve evidence, not a reason to stop dispatching.

### Verified merge and resource cleanup DAG（单源单汇）

一个交付/resource 回收任务必须以唯一入口进入、唯一终点收口；节点内部可以是状态机，但独立功能之间不得跨节点回边改真源。

**peer 可直接 merge + push，当且仅当：**

1. merge 前 fetch 最新 `origin/main`，确认远端 main 没有比自己主链更新的提交；
2. 候选已 rebase/组合到该最新 main，并重跑过受影响验证 + 独立 review PASS；
3. 当前项目没有 pending merge/release 锁（简单 queue 文件或 daemon 状态）；
4. merge 后立即核对本地 main 与已验候选等价，并确认 `origin/main` 回执；
5. 任何失败（合并冲突、push 拒绝、CI 未跑通）都必须停下并上报 master，不允许强行推进。

否则 merge 的 owner 仍是 current master；review PASS 不授予流程外发布或生产变更。

### Collab runtime delivery DAG（单源单汇）

适用范围：Collab server、daemon、CLI、MCP、route、identity、notification、wake、
mailbox、task lifecycle 或安装包的修复/发布。源码测试和 `git push` 都不是终点；
runtime 节点未完成时必须报 `INCOMPLETE`。

```text
入口: latest_origin_main_candidate
  -> applicable_targeted_tests_pass
  -> scripts/install-global-collab.sh
  -> installed_binary_version_digest_verified
  -> collab_down_exact
  -> collab_up_exact
  -> collab_context_registered_live
  -> collab_mcp_initialize_matches_version
  -> live_replay_or_explicit_non_applicable_reason
  -> applicable_architecture_review_pass_after_behavior_verified
  -> commit_merge_push_verified
终点: cleanup_verified
```

维护窗口只使用 `collab down` 和 `collab up`。不得用 broad kill、第二个 daemon、
旧 binary、跳过重启或手工 route/identity 文件替代节点。重启后必须同时记录旧/新
PID/socket、installed binary SHA256、MCP version、context worker/role/transport、
pending_merges/worktrees 状态；任一项不匹配都不能声称交付完成。

**peer 资源回收单源单汇：**

- 入口：本任务生命周期终点；
- 终点：本任务创建且确认不再需要的资源全部移除，并留下核对证据；
- 范围：worktree、playground、临时文件、日志、进程、forward；
- 核对：`git -C <repo> worktree list --porcelain` 不含本任务 worktree；`test ! -e <playground>` 成功；本轮 tmp/日志/进程已移除。

**master 资源回收单源单汇：**

- 入口：调度/交付收口；
- 终点：idle/stale 资源核销：过时 playground、失效 worktree、dirty main、已审查完成或明确 drop 的分支/候选/交付记录，逐个按 review/commit/drop 的授权终点核销并留证据；
- 边界：只回收已确认 stale 且有 owner 证据的资源，不得跨节点直接删除他人资源、共享 playground、共享进程或未经核销的候选；
- 失败：任何回收失败必须显式报告原因并标为未收口，不允许静默跳过。

A worker or subagent executes only the approved assignment, owns that
task's full lifecycle, and returns evidence. It has no global schedule.
On a blocker: find a concrete solution first, then report it to the current
master immediately. Do not wait. Do not lazy-think (symptoms without a
fix, or idle hoping master will design it). Copy parent if parent is not
master. If delivery or test conditions are unclear, propose the missing
conditions instead of guessing. Only an explicit `collab notify unsubscribe
<subscription-id>` stops a default direct-message lease; finishing or closing
the last task never cancels it, so a registered peer stays wakeable. There is
no separate `collab notify close` command. Do not unsubscribe another peer's
lease. After an explicit unsubscribe, re-arm with `collab notify subscribe
--event direct-message`.

AGY review is not used for Collab v1 lifecycle gates. Ordinary review uses an
independent review path when review is required; milestone review uses Codex
Review with the `oauth` profile and `gpt-6.1-sol`. Astra is not a reviewer.
Missing AGY is not a blocker because it is excluded; missing a
declared review gate is a failure.

Without an available, verified selected transport, initialization fails
explicitly. AppServer is preferred when its candidate passes the owner
self-check; a tmux-only binding remains a separate selected transport and is
never substituted for an AppServer binding. Read-only journal/mailbox queries
remain available where their command permits unregistered access. Pane output
is an observation, never task or control truth.

| Intent | Command |
|---|---|
| Bootstrap, recover, or re-locate this agent; read peers, master, scheduling state, and env | `collab context` |
| Notify a peer now | `collab sendmessage --to <peer> --subject <short-topic> "<original message>"` |
| Receive and consume notifications | `collab recv` |
| Read one notification without consuming | `collab msg <notification-id>` |
| List unread messages | `collab inbox` |
| Recover an already-delivered notification | `collab ack <id>` or `collab ack --all` |
| Inspect worker health and notification status | `collab worker status [id]` |
| List peers (operator diagnostic; also in `collab context`) | `collab who` |
| Check own subscriptions | `collab notify status` |
| Inspect current master (operator diagnostic; also in `collab context`) | `collab master status` |
| Replace the current master with an explicitly approved promote | `collab master promote --approval "<user text>"` |
| Clear the current master authority with explicit approval | `collab master clear --approval "<user text>"` |
| Delegate master authority to another peer (current master only) | `collab master delegate <peer>` |
| Split work to a registered peer | `collab sendmessage --to <peer> --subject <topic> "<assignment with delivery and test conditions>"` |
| Report a blocker to the current master | `collab sendmessage --to <master> --subject blocker "<task_id; cause; proposed fix; decision needed>"` |
| Cancel one of your own notification leases | `collab notify unsubscribe <subscription-id>` |

After a transport preview, use its notification ID and abbreviated subject to weigh
urgency against the current task. When selecting the notice, run
`collab msg <notification-id>`, read durable detail, and execute the actionable
request inside this Agent's scope. Do not stop at ACK or waiting; mailbox truth
persists.

### Situation -> action (one entry)

`collab context` is the single agent entry. It resolves the canonical root,
creates a missing baseline, starts a stopped daemon, restores identity and
registration, re-arms the default direct-message lease, and returns the
authoritative snapshot plus the current role's `operations`.

That one snapshot is also the agent's complete state read. It carries the
projections that used to require separate calls, so no agent flow needs to run
`collab who`, `collab status --all`, `collab master status`, or a shell
`env | rg` probe first:

| Snapshot field | Replaces | Contents |
|---|---|---|
| `master` | `collab master status` | current master authority projection (holder, scope, approval) and `master_wake`; transport liveness is a separate fact |
| `peers`, `peer_count` | `collab who` | every registered peer with role and presence |
| `summary`, `master_wake`, `subagents` | `collab status --all` | worker/message/task/subagent counts and the scheduling state |
| `pending_merges` | `collab status --all` | durable merge obligations |
| `env` | `env \| rg '(COLLAB\|APPSDK\|CODEX\|HOME\|USER)'` | the identity-relevant variables of this agent's own process; credential-shaped names (`*TOKEN*`, `*KEY*`, `*SECRET*`, `*PASSWORD*`, `*CREDENTIAL*`) are dropped, so `CODEX_API_KEY` never appears |
| `operations`, `next_actions` | — | the current role's required actions |
| `requires_identity_update` | — | present only when the agent must repair its identity; see below |

| Situation | Do this | Never do this |
|---|---|---|
| First time in a project | `collab context` | a separate identity probe, or `collab init` used as a substitute for `collab context` |
| Another worker or project claims your tmux pane, or the recorded pane route is stale | `collab context`; the daemon replaces the claim by default | inspect routes or archives, pick a worker id, or edit any route/identity file |
| Master authority must move to this peer | `collab master promote --approval "<user text>"` | promote without explicit user approval |
| Master authority must be cleared for this project scope | `collab master clear --approval "<user text>"` | clear without explicit user approval; or expect clear to delete tasks, messages, peers, or bindings |
| Thread/session changed, or after daemon restart | `collab context` (daemon reconciles identity in place) | a manual identity recovery command, `collab down`/`up` |
| Need peer list, master state, scheduling state, or env | read them from the single `collab context` snapshot | call `collab who`, `collab status --all`, `collab master status`, or grep the environment as a separate step |
| `requires_identity_update` is present | read `required_fields` and `exact_error`; when the returned action is the factual supplement, run that one action with only the requested `session_id`, `thread_id`, `endpoint`, or `namespace` from their real source | select or guess a worker; run an identity selection or recovery command, or a status/route/init hunt; set an identity override or approval supplement |
| `collab context` exits non-zero with `COLLAB_CONTEXT_UNRESOLVED` | preserve the error, run `collab context` from the canonical main tree | edit routes/token state, copy identity, reset the project |
| Default lease looks stopped | `collab context` re-arms it unless the owner explicitly unsubscribed | probe sockets, call a transport directly |
| Notification arrived | `collab msg <id>`, then act; `collab recv` consumes | ACK-only, or treat submission as consumption |
| Master has a pending merge | `collab context` → `pending_merges`, merge the candidate, then `collab task integrated` | rely on a remembered message, or try to close first |

Read-only operator diagnostics remain available for human maintenance and
audit: `collab who`, `collab status --all`, `collab worker status`, `collab
route resolve`, and `collab master status`. They are not agent identity
recovery steps. `collab down`/`up` require explicit human authorization.
`collab init` is not an agent bootstrap and not a second identity algorithm: it
drives the same daemon identity gate as `collab context`, and it remains the
documented entry for existing AppSDK init consumers. Ambiguous and
cross-project anchor matches stay fail-closed on both entries and need human
adjudication; no entry overrides them.

### Context 状态与终点（DAGpipe：单源单汇）

`context_request` 是唯一入口，`state_snapshot` 是唯一成功出口，中间节点按
DAGpipe 顺序执行：解析项目根 → 检查/创建基线 → 检查/启动守护 → daemon 身份门
→ 环境投影 → 输出快照。CLI 自动观察可用事实；daemon 身份门负责选择、创建、
恢复、更新身份，持久化 credential/binding/lease，并返回完整快照。缺失事实是本次
调用的终态：`registered=false`、`identity=null`，`requires_identity_update`
给出精确 `required_fields`、`reason`、`action` 和 `exact_error`。Agent 只按
`required_fields` 使用一次 `collab context --provide` 补充真实事实。没有 pending
workflow，没有第二个修复入口。根解析失败和显式冲突/错误保持原错，不伪装为成功快照：

| 状态/终态 | 含义 | Agent 动作 |
| --- | --- | --- |
| `state_snapshot` | 引导成功；含 role/operations/master/peers/peer_count/summary/master_wake/subagents/inbox/worktrees/tasks/env | 读快照执行当前角色的 `operations`；无需再跑 who/status/master status |
| 陈旧/他人的 pane claim 已被顶掉 | 默认路径的一部分，不产生额外终态 | 无；不需要第二条命令，也不参与裁决 |
| `identity_update` | daemon 需要缺失事实或报告显式身份冲突；`registered=false`，`requires_identity_update.required=true` | 只读 `required_fields`/`reason`/`action`/`exact_error`；若 action 是 factual supplement，则用一次 `collab context --provide` 提供请求的四个 scalar keys；显式冲突保留原错，不选择 worker，不跑 status/route 恢复，也不把 `collab init` 当第二次修复尝试 |
| `COLLAB_CONTEXT_UNRESOLVED` | 无 route、无 baseline、无 git 根（非零退出） | 保留错误，改在 canonical main 再跑 `collab context` |
| 拒绝在 playground 创建基线 | 在 worktree 内引导（非零退出） | 回到项目 main 根执行，不删旧身份 |
| 默认订阅已停 | owner 显式 unsubscribe 持久生效 | 需要再收消息时用 `collab notify subscribe --event direct-message` 重订阅 |

`requires_identity_update.reason` is a typed code. It is not prose and it is not
permission to select a worker. The supplement has exactly the four scalar fields
`session_id`, `thread_id`, `endpoint`, and `namespace`. A value that conflicts
with an automatically observed fact is `IDENTITY_FACT_CONFLICT`, not an override.
Explicit conflicts, token rejection, route failure, runtime-binding failure, and
transport failure keep their original error and non-zero exit; no fallback or
pending workflow conceals them.

完整语义图、转移表和 owner 映射见 `docs/collab-context-state-machine.md`；
机器可校验 SESE 图见 `docs/dagpipe/collab-context.graph.json`
（`dagpipe graph validate`）。

## Initialize once

The automatic state entry is always:

```sh
collab context
```

The daemon-owned identity context completes the bootstrap. A conflicting claim
on your tmux pane is replaced by the daemon inside this same call, so agents do
not adjudicate pane conflicts and do not run a second command for them.
`collab init` is not a second agent entry and not a second identity algorithm,
but it drives the same daemon identity gate and stays the documented entry for
existing AppSDK init consumers. Neither `collab init` nor AppSDK init creates,
changes, or clears master authority; init is not an implicit authority reset.
Agents do not run a separate identity, route or archive probe, or a manual
subscription, as part of bootstrap.

## AppServer runtime registration

Registration derives the host namespace from the active runtime. The exact
`CODEX_INTERNAL_ORIGINATOR_OVERRIDE=Codex Desktop` marker selects `codex_app`;
`Codex CLI` or `Codex TUI` selects `codex_tui`, and `TMUX_PANE` selects
`codex_tui` when no originator marker is present. An explicit
`COLLAB_APPSERVER_NAMESPACE` is reserved for nonstandard hosts and must be one
of those two values. Unknown host markers never select an AppServer namespace.
With an explicit AppServer socket they fail registration; without one, the
adapter returns no AppServer candidate and may continue through another
existing transport only after that transport's normal verification. Never
default an unidentified session to `codex_tui`.

Endpoint discovery is host-specific and performed by the Collab client adapter
before registration. `COLLAB_APPSERVER_SOCKET` and
`CODEX_APP_SERVER_SOCKET` are explicit endpoints. For Desktop only, the client
adapter may discover the managed endpoint at
`$CODEX_HOME/app-server-control/app-server-control.sock`, or
`$HOME/.codex/app-server-control/app-server-control.sock` when `CODEX_HOME` is
unset. A TUI must supply its own reachable AppServer endpoint; never borrow the
Desktop managed socket for a private embedded TUI. `CODEX_APP_TOOLS_PIPE_PATH`
is a tool pipe, not an AppServer endpoint. The daemon still verifies the exact
thread/session/project tuple before committing the route.

The selected namespace is persisted with the recipient route. `turn/start`
notifications use that stored recipient namespace (`codex_app` or
`codex_tui`); they never infer it from the sender or hardcode the TUI namespace.
If an AppServer endpoint is unavailable before registration, the existing
transport selection rules apply. Once an AppServer binding is selected, do not
switch transports after an RPC error.

## Worktree identity

A Git worktree is a task execution directory, not a second identity or a
substitute for the canonical project root. `collab context` resolves the
canonical root automatically and the daemon owns identity selection, creation,
recovery, update, route publication, and binding persistence. Do not create a
worktree-local peer. External linked worktrees use the registered Git main root
automatically for context and ordinary commands; do not switch cwd to recover
an identity. The `collab context` snapshot is the agent's complete
master-authority and peer read. A failed context is not evidence that no master
exists; preserve the exact error. Master promotion requires explicit user
approval and an authenticated current binding, not the incumbent's or the
candidate's liveness. `collab master status` remains a
read-only operator diagnostic.

### Native thread route resolution

The daemon is the sole route selector. The current AppServer owner plus exact
session/thread binding identifies a route. Historical routes and tombstones
are not candidates. The daemon verifies the selected owner with native
`thread/read`; a tmux tuple can recover identity only when both runtime IDs are
missing and never substitutes for communication or presence.

`collab route resolve` exposes a read-only operator diagnostic for the current
native thread; `--native-thread-id <id>` and `--session-id <id>` must match the
live runtime binding. It is not an agent identity recovery step. The daemon
returns exactly one route, `ROUTE_RESOLVE_NOT_FOUND` for zero matches, and
`ROUTE_RESOLVE_AMBIGUOUS` when multiple current bindings match. Missing, unknown,
mismatched, or malformed endpoints fail explicitly. The resolver is read-only
and returns no token.

## Subscribe to a future event

Use subscriptions only when this Agent wants a later event to wake it:

```sh
collab notify subscribe --event resource-released --subject <resource-id> \
  --ttl-seconds <bounded>
collab notify subscribe --event deadline --subject <timer-id> \
  --ttl-seconds <bounded>
```

The sender never inspects or configures the recipient's subscription.
For subscription semantics or delivery diagnosis, read
[references/notifications.md](references/notifications.md).

Desktop does not register goal subscribe. Goal subscribe is master-only
long-horizon scheduling; workers and Desktop clients must not infer or
register it.

## Hard boundaries

- Never attach production work to v2 or `.agent-collab-v2`.
- Project scope comes from the exact process cwd. Never choose/search/hardcode
  a path. MCP and child commands inherit the same environment.
- Identities are equal peers by default. There is no implicit master from
  first registration, automatic process recovery, or inferred `/goal`. Collab
  master is explicit, user-approved project arbitration; it is not Codex root,
  and it does not take ownership of another peer's task. The authority owner is
  the current typed grant in the exact project and app scope, with only empty
  or assigned states; the current holder controls independent of its transport
  liveness. An explicitly user-approved `collab master promote` replaces the
  recorded holder, and only the current master grant holder may
  `collab master delegate`. Independent peers may
  decline a master collaboration invite; managed subagents must obey the
  master. Master splits by dependency then unique write scope, assigns
  subagents with unambiguous delivery and test conditions, and keeps
  architecture/integration/acceptance; it does not take another peer's
  task. Workers and subagents find a solution first, then report blockers
  to master immediately; they do not wait or dump symptoms. Last owned
  task close keeps that owner's auto-notify armed; use
  `collab notify unsubscribe <subscription-id>` for a specific leftover
  lease. Only an explicit unsubscribe stops it. AGY review is not a Collab v1 gate; ordinary review is independent,
  and milestone review uses Codex Review (`oauth`, `gpt-6.1-sol`). Explicit managed subagent tasks
  use the task-bound inspect obligation above; it is not a free-form task queue
  and does not create worker transport input.
- Each peer owns its complete task/worktree/integration/resource/cleanup
  lifecycle. Never mutate or close another peer's work.
- Send only explicit notices, shared-resource coordination, or subscribed async
  results—not routine progress, heartbeat, ACK, review, or completion reports.
- A wake is only a signal. It cannot change task/resource truth, fabricate
  success, authorize maintenance, or create an ACK loop.
- `absent` or `unknown` liveness for the selected binding produces no transport
  input. If its endpoint is unreachable, unowned, or mismatched, the
  subscription enters its explicit unavailable state to prevent storms. There
  is no cross-transport fallback after binding selection.
  Each due batch is reserved durably once; failed or uncertain attempts are
  never automatically replayed, including after restart. Details remain
  readable in the inbox.

## Load details only when needed

- Task/worktree registration, delivery, close, cleanup:
  [references/task-worktree-lifecycle.md](references/task-worktree-lifecycle.md)
- Resource conflicts and bounded waits:
  [references/resource-waits.md](references/resource-waits.md)
- Migration, daemon stop/start, deprecated commands:
  [references/migration-daemon.md](references/migration-daemon.md)
- Source/release/install/restart verification:
  [references/verification.md](references/verification.md)

Do not load references for ordinary `sendmessage`, `msg`, or `inbox`.

The current v1 contract has no 15-minute periodic worker liveness. Worker and
master liveness wake only on a supported timer/wake, direct message, or real
external event. If any installed copy still contains the old wording, replace
it from this SKILL and its references.
