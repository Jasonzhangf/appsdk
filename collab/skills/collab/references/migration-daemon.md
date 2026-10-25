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
mailboxes and daemon runtime state. Project-local `.agent-collab/` may provide
project discovery. Its retired server journals and mailbox files are not imported
or replayed into the current runtime and remain on disk. Historical identity
compatibility within the current central store remains context-owned;
project-local state is not a second host truth. AppSDK `.appsdk/` and `.appsdk-control/`
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
for an AppServer endpoint or an MCP invocation, guess a socket, delete a journal,
or edit identity state. Report source, global installation and actual Desktop
delivery separately. Service restart restores persisted bindings after native
verification; agents do not repeat registration to repair a bridge process.

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

## AppServer runtime registration

Register only the AppServer that serves the current caller's active session and
thread. The endpoint, session ID, and thread ID must come from that caller's
own runtime; never submit another agent's or worker's AppServer, session, or
thread, even if it is reachable. If ownership of the exact endpoint and tuple
cannot be established, fail registration rather than borrowing another
runtime's identity.

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

For the AppServer branch, the Collab client accepts only `COLLAB_APPSERVER_SOCKET` or
`CODEX_APP_SERVER_SOCKET` supplied by the current runtime as the endpoint for
the current session. In particular, never derive
`$CODEX_HOME/app-server-control/app-server-control.sock` or
`$HOME/.codex/app-server-control/app-server-control.sock`: those are
host-managed paths, and their existence, owning daemon PID, or ability to list
the thread does not prove that the registering client uses that AppServer.
Never copy a parent agent's, another process's, or a merely host-managed
endpoint into registration or `collab context --provide`. A matching
`thread/read` result proves that an endpoint can access the thread; it does not
prove that the current client uses that AppServer. If the current runtime does
not provide its endpoint, do not register an AppServer route or guess one; use
another already verified transport only when it belongs to this peer, otherwise
report that registration lacks a current-session endpoint. `CODEX_APP_TOOLS_PIPE_PATH`
is a tool pipe, not an AppServer endpoint. Desktop uses a runtime-shared bridge:
CLI automatically enrolls its current session/thread, and native Desktop verifies
the canonical project before the bridge publishes a scoped capability. Native
service calls use explicit correlation IDs, which do not claim agent turns. The daemon
connects only that bridge UDS; it does not connect the tools pipe or require
a nonexistent AppServer endpoint. Runtime service correlation remains separate
from agent execution evidence and the message sender. See
[current recovery facts](stable-name-recovery.md).
The daemon still verifies the exact thread/session/project tuple before
committing a supplied route.

The AppServer namespace is persisted with the recipient route. Its `turn/start`
notifications use that stored recipient namespace (`codex_app` or
`codex_tui`); they never infer it from the sender or hardcode the TUI namespace.
If an AppServer endpoint is unavailable before registration, the existing
transport selection rules apply. Once an AppServer binding is selected, do not
switch transports after an RPC error.

## Reset is explicit discard, not recovery

Only use a project reset when the user explicitly requests discarding that
project's Collab epoch and the caller holds the current project master grant:

```sh
collab reset --project
```

Run it from the current project. The daemon authenticates the current scoped
master, then clears this project's messages, tasks, bindings, routes,
subscriptions, operations and assets, deletes project history, and releases its
load. It preserves other projects, AppSDK state and the global daemon.
Historical pane owners, open work and resident host storage do not block this
authorized project reset. The next `collab context` creates a peer with no
inherited master grant. Real deletion or durability errors return incomplete and
retain the scoped reset intent for recovery. No legacy discard mode or separate
approval argument exists.
It is not a prerequisite for context recovery, an identity-conflict remedy or a
history-preserving migration. Host maintenance is outside the agent interface.
Do not imitate reset by editing files.

`.appsdk/` and `.appsdk-control/` are AppSDK-owned and are not removed by
`collab reset`. The host-wide runtime truth remains `~/.collab/`; project-local
`.agent-collab/` is reducer input and local durable data, not the global truth.
An AppSDK `reset-governance` or `appsdk init --fresh --discard-legacy` is not a
Collab reset/migration and must not remove `.agent-collab/`. Never manually
remove `.agent-collab/server/journal.jsonl`, mailbox files, identity tokens,
task records, or bindings to make status look clean. `collab ack` stays a
compatibility operation; it is not a substitute for task close or identity
recovery. The two operations have separate transactions and separate completion
evidence.

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

The authority owner is the current typed grant in the exact project and app
scope, with only two states: empty (no current grant) and assigned (one current
holder). An approved promote atomically replaces any recorded holder and is not
gated on the incumbent's or the candidate's transport liveness; a tmux pane and
an AppServer oracle are addresses, not credentials. Clear removes only the
scoped grant and never deletes tasks, messages, peers, or bindings. Context
never auto-promotes and never infers authority. An authenticated peer with
explicit approval may clear the scoped grant; the caller does not need to be the
master first. An internal init adapter result alone never proves master
ownership, and AppSDK init is not an implicit authority reset.

Do not use legacy `collab role`, `collab master recover`,
`collab transfer-master`, `collab task claim`, `collab task dispatch` or
`collab remove-worker` as recovery steps. Managed-subagent compatibility
records do not grant execution authority. Master and peer both orchestrate
long-horizon goals at different levels; one-shot execution goes to subagents.

## Peer lifecycle CRUD and read-only observation

`collab context` returns a `peer_lifecycle` operation card only to the current
master. It contains the exact commands, scope rule, success receipt, and failure
handling. Use it as the command contract. A peer with no current master grant
does not manage other peers.

All targets must belong to the same registered canonical project `main` and app
scope as the master. The requested `cwd` may be the main checkout or any
existing directory/linked worktree in that same project. Collab resolves the
worktree to its registered `main`; directory nesting or a matching path prefix
does not establish project identity.

| When | Command | Required result |
|---|---|---|
| Create a peer | `collab worker create <peer-id> --cwd <existing-project-dir-or-linked-worktree> --op <stable-operation-id> [--model <model>]` | `outcome=complete`; response identifies the real thread, route, registration, and verified ready turn. Reuse the same operation ID to recover a retry; never create a replacement ID after an unknown result. |
| Inspect a peer | `collab worker read <peer-id>` | `outcome=ok` with the exact peer lifecycle projection. `collab worker read` with no ID reads the caller. |
| Move a peer's AppServer cwd | `collab worker update <peer-id> --cwd <existing-project-dir-or-linked-worktree>` | First call omits `--op`. `outcome=complete` requires settings update and cwd readback. Supply `--op <returned-operation-id>` only to retry/query that retained update after an uncertain result. |
| Unregister yourself | `collab worker close --self --reason "<auditable reason>"` | The authenticated current pane may retire its own identity even with outstanding tasks. `outcome=complete` requires self binding and route retirement; `runtime_archive=not-applicable`. The host process, tasks, messages and history are preserved. Only this identity's exact master grant is revoked. |
| Close another peer | `collab worker close <peer-id> --reason "<auditable reason>"` | First call omits `--op`. `outcome=complete` requires verified runtime archive and peer, binding, route, lease, and subscription retirement. Open responsibilities are refused. Use `--op <returned-operation-id>` only for its retained retry/query. |
| Read an uncertain operation | `collab worker query --op <returned-operation-id>` | Reads the retained outcome without repeating a host effect. Do not start a replacement operation while the result is unknown. |

For self-unregister, the first call also omits `--op`; use the returned operation
ID for query or retained retry. Self-close does not stop the host process or
delete tasks, messages, claims, pending merges or history. Unread messages
remain durable. A later `collab context` at the same live anchor rebuilds a
default peer with a current binding and route. It does not restore the retired
master grant.

Any authenticated peer of the same registered project may read the latest live
buffer of an exact registered tmux peer. Master authority is not required
because the read changes no durable state.

| When | Command | Required result |
|---|---|---|
| Decide whether to keep waiting, escalate, or steer | `collab snapshot <peer-id> --lines <1..200>` | Returns the latest bounded, transient tmux pane buffer through one presence check and one `capture-pane`. It sends no input, consumes no mailbox message, bumps no revision, and writes no durable snapshot receipt. AppServer and DSH peers remain unsupported. |

`collab worker snapshot <peer-id> --lines <1..200>` remains a compatibility
alias. The buffer is only the latest visible pane text. It does not prove
busy/idle, Codex turn state, message consumption, or task completion.

For `refused`, `partial`, or `unknown`, keep the typed response and its
operation ID. A responsibility or project-scope refusal has no lifecycle
success; fix the named condition before a new operation. An unknown or partial
result is not success: query that operation and do not claim the peer was
created, updated, or closed until its verified terminal receipt says so.

Read-only operator diagnostics remain available for human audit: `collab who`,
`collab status --all`, `collab worker status`, `collab
route resolve`, and `collab master status`. They are not agent identity
recovery steps. User daemon maintenance is outside the agent interface.
`collab init` is not an agent bootstrap and not a second identity algorithm: it
drives the same daemon identity gate as `collab context`, and it remains the
documented entry for existing AppSDK init consumers. Historical pane claims
do not override the current cwd/session or block peer
restoration. Optional user authorization replaces the current project master
through the existing authority owner.

## Global CEO and project master

CEO 是宿主全局唯一的跨项目协调者。各项目 master 继续负责本项目目标、peer 分工、
验收和交付，旧项目合同保持原义。工作角色 `ceo` 不授予全局权限；只有 daemon 的
typed CEO grant 才有效。全部规则仍属 Collab Skill 范围，不新建 CEO、master 或
角色 Skill。

先在用户选定的 workspace 注册真实身份，再按用户明确批准授予 CEO。已有 workspace
直接复用，不迁移或覆盖；没有既有配置时使用 `~/.collab/workspace`。目录存在不是
CEO 已授权的证据。`collab ceo status` 只读查看当前 grant 和默认 workspace，不注册。

```sh
collab ceo status
collab ceo promote --approval '<用户明确批准原文>' --workspace '<当前已注册workspace>'
collab ceo clear --approval '<用户明确撤销原文>'
collab ceo progress
collab ceo progress --project '<目标项目根目录>'
```

context 返回当前全局 CEO 联系方式。只有当前 grant 持有者才获 CEO 操作与职责；
`identity.role` 和项目 `authority` 保持原义。正常会话续接沿现有稳定名字 owner 规则，
不能凭旧 session、消息正文或角色字符串重建权限。没有 CEO 投影或入口不受支持时，
报告安装/运行差异，不声称已启用、不手工改状态、不管理全局 daemon 生命周期。

CEO 在自己的 workspace 调用下列入口。目标由 daemon 解析为该项目当前 master，
新请求不取消 master 原有任务，不改已有 owner、scope、固定执行者或必需 reviewer。

```sh
collab ceo send --project '<目标项目根目录>' --subject '<主题>' --reply-required '<需要实际执行或回答的正文>'
collab ceo send --project '<目标项目根目录>' --subject '<主题>' --reply-on-completion '<完成后交回的要求>'
collab ceo append-task --project '<目标项目根目录>' --id '<任务ID>' --title '<标题>' --description '<范围与授权>' --delivery-condition '<交付条件>' --test-condition '<验收条件>'
```

append-task 追加到目标项目现有 board，由当前 master 负责，并默认要求完成后回复。
不要另建 CEO 业务任务表，也不要把发送消息当作任务板已写入。回执只证明其实际字段：
持久入队、通知提交、master 已安排、交付和回报各自分开。

项目 master 收到 CEO 请求后，落实授权 action，安排 ready 工作，并向 CEO 交回实际
结果、进度或具体阻塞。使用自己的项目身份，不切到 CEO 的目录冒充其身份：

```sh
collab ceo reply --subject '<主题>' '<普通进度或具体阻塞>'
collab ceo reply --subject '<完成主题>' --in-reply-to '<原请求ID>' '<实际交付与证据>'
```

普通进度不带原请求引用，保留完成后回复义务。CEO 经用户批准换人后，原请求的完成
回复仍用原请求 ID，`ceo reply` 将结果交给现任 CEO，不手工改历史发送者或回复标记。
真正完成后才以原请求 ID 回复；daemon
在正确 master 的回信持久送达后清除精确原标记。收到 pending-reply idle 提醒时处理
原请求，不回复提醒本身。回交后继续自己的任务；无可执行工作时按既有流程交回/申请
idle。项目 master 不因全局 CEO 存在而放弃本项目交付责任。

### 目标项目没有 master

未知、不可达或发送失败不等于没有 master。只有当前控制真源明确无 master，才向用户
说明目标路径、profile（默认 gcm）、现有 tmux socket/session 与新会话用途，并取得
本次明确启动批准。不要把“实施 CEO 功能”的开发授权当作生产新建 agent 的批准。

```sh
collab ceo bootstrap-master --project '<目标项目根目录>' --approval '<本次用户启动批准原文>' --profile gcm --tmux-socket '<已有socket>' --tmux-session '<已有session>' --op '<本次operation>'
```

该入口只在现有 tmux server 的指定 session 创建自有窗口，启动 Codex YOLO；用户可
指定其他 profile。不能注入假 Codex session ID，不能直接伪造身份、授权或收信。若无
现有端点，报告具体缺口；不启动、重启或终止 tmux server，不关闭用户已有窗口。

新 master 的初始提示要求它在真实会话内依次完成：`collab context`；按批准原文执行
`collab master promote --approval '<原文>'`；再给 CEO 发真实就绪握手：

```sh
collab ceo bootstrap-ready --op '<本次operation>' --subject '<就绪主题>' '<真实注册、master授权与可收信结果>'
```

仅窗口已创建时状态是 launched/pending。daemon 认证目标项目当前 master 并将握手
持久送达 CEO 后才是 ready。同一 operation 不重复建窗口；结果未知保留现场，不盲目
重试或自动删除。需要放弃本次启动协调时，先向用户说明已知窗口和未决结果，取得明确
取消批准，再执行 `collab ceo bootstrap-cancel --op '<本次operation>' --approval '<本次取消批准>'`。
取消只关闭协调意图，不停止 Codex、不关闭 pane、不撤销已存在的 master；回执中的
`process_stopped: false` 必须如实报告。确认旧尝试不会继续后，用户另行批准的新启动
使用新 operation，不能把取消回执当成已清理进程。已 ready 的操作不能用此入口撤销。
CEO 收到握手后，继续执行尚待派发的 send/append-task，不能到“创建
成功”就停下。进度用 `collab ceo progress --project '<目标项目根目录>'` 查看。

CEO 下一步提示词：先执行 `collab context` 和 `collab ceo progress`，读取当前授权、
项目 master 与任务回报。在已有授权内追加目标、要求实际进度并处理回信；项目没有
master 时先取得本次启动批准。收到真实就绪握手后继续派发待办。保留项目 owner 和
用户指定的执行/review 约束，向用户报告实际安排、交付与尚待决定项，然后继续推进。

## Cross-project master communication

Only the current project master grant holder may use this channel. The target
must be the current master grant holder of another initialized project;
non-master peers and managed subagents are rejected before any message is
persisted.

### 权限、目标解析与下一步动作

这是一条已存在的项目 master 到项目 master 通道。工作角色（designer、reviewer）
不授予此权限。普通 peer 把跨项目请求交给本项目当前 master；不得换 cwd 冒充目标
项目的 agent，不得直接写 mailbox、猜 session ID，或通过另一个宿主通道绕过拒绝。
全局 CEO 走独立授权通道，不把 CEO 称呼当作项目 master 授权。

发送者在自己的项目执行 `collab context`，使用返回的当前身份和 master 授权。
用户提供目标项目绝对路径后，可以只读查询目标当前 master；这个查询不注册身份：

```sh
(cd '/目标项目绝对路径' && collab master status)
```

读取回执的 `master.worker_id` 作为 `--to`，不要按 Jenny 等显示名搜索历史 session。
查询失败不代表没有 master。已有 master 但未知、不可达或投递失败时，保留原错，
不创建替代 master。只有真源明确无 master，才向用户说明项目路径、profile 和启动方式，
经确认后创建新的具名 tmux Codex session 并授权 master。默认 profile 为 `gcm`，
使用用户已确认的 YOLO 模式；不得关闭现有 tmux server 或 pane。

从发送者自己的项目目录执行下列命令；`--project` 只指定接收项目，不改变发送者身份：

This existing master-to-master channel is master-only and explicit. Only the
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

接收 master 读取完整原消息并执行已授权动作。非 DSH 宿主用 `collab recv`；
DSH 直接处理 gateway 已投递的正文，不再 Poll/ACK。回信仍在自己的项目执行
`collab master send`，将原消息 `from` 中的源项目用作 `--project`、源 worker ID
用作 `--to`；当前 CLI 的这条命令尚不支持 `--in-reply-to`，在正文中写明原消息 ID。
这只提供可读关联，不宣称已清除 daemon 回复标记。持久入队、宿主投递、接收消费和
实际回信分别核实，发送成功不能证明对方已经执行。

可直接给 agent 的下一步提示词：

> 先在自己的项目执行 collab context，核对你是当前 master。对用户给定的目标项目
> 用 `(cd '<目标项目>' && collab master status)` 只读获取 master.worker_id。
> 回到自己的项目，用 `collab master send --project '<目标项目>' --to '<worker_id>'
> --subject '<主题>' '<具体action、范围、验收和回信要求>'` 发送。不要搜索历史 Jenny
> session，不要用 sendmessage 跨项目投递。保存 msg_id，分别报告已入队、已消费和
> 已收到实际回信的证据；只获得哪一步就报告哪一步。拒绝或未知状态保留原错。
