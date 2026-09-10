# AppSDK 内部通信与长程 Loop 设计

## 目标、owner 和边界

AppSDK 自己拥有 `appsdk-comm/v1` 的通信协议、身份与 scope、路由授权、通知投影、
事实记录、错误链和长程 Loop。Desktop、TUI 或其他宿主只通过 JSON 调用面接入；
宿主 appserver 可以展示或执行返回的 intent，但不能替 AppSDK 伪造执行结果。
`/Users/fanzhang/Documents/github/codexapp` 只提供通信语义参考，不是运行时依赖，
也不属于本模块的修改范围。

`mailbox`、`tmux` 和 `appserver` 是同一个通信抽象的承载 adapter，不是三套协议或
三份事实：

- `mailbox` 是内置的持久化 adapter。通信事件追加到项目下
  `.appsdk-control/communication/mailbox.jsonl`；adapter 返回 `accepted` 只表示
  AppSDK 已持久化，不表示收件人已读取。
- `tmux` 只承载有界的唤醒或摘要。adapter 必须绑定收件地址；`execute=false` 只
  生成 `intent`，只有显式 `execute=true` 才允许调用 `tmux send-keys`，并在命令
  成功后返回 `delivered`。
- `appserver` 只向绑定的 endpoint 返回 `intent`，receipt 带
  `hostMustExecute=true`。宿主没有回报执行证据前，状态不能升级为
  `delivered`、`executed` 或 `read`。

本模块不启动第二个 daemon，不读取外部 Collab、mailbox CLI 或 tmux 状态，不把宿主
的 session、模型名或 endpoint 推断成角色。adapter 失败必须保留原始错误和事实，
不能用 fallback 伪造成功。

## 事实、投影和并发写入

JSONL 是唯一持久化事实源。每行是带 `protocol`、`eventId`、`at`、`kind` 和 `data`
的事件；消息、状态、adapter receipt、通知、唤醒、Bug、Loop 和错误都追加到同一
份记录。读取时从头重放得到 `status` 投影；投影不是第二份可写事实。

同一个 mailbox 命令生命周期使用独占 `.jsonl.lock`：先取得锁，再重放、校验和追加，
最后同步写入。锁被占用返回 `communication_busy`，不在旧投影上继续写。JSONL 坏行、
协议不匹配或未知事件不跳过，统一以 `journal_corrupt` 或
`journal_unknown_event` 失败。

`message.created`、`message.state`、`notification.queued`、`notification.delivery_attempt`、
`notification.emitted`、`notification.batch_emitted`、`notification.delivery_failed`、`wakeup.reminder`、
`bug.*`、`loop.*` 和 `error.recorded` 是可重放事件。错误处理也必须追加事实；如果
错误事实本身写入失败，返回包含主错误和次级写入错误的错误链。

## 地址、scope、角色和 lease

通信地址固定为 `scopeId/sessionId`。Scope 记录 `appserverId`、宿主声明的
`namespace`（`codex_app` 或 `codex_tui`）、由 cwd 确定的 `projectRoot` 和允许的
`sessionIds`。不同 `scopeId` 即使 appserver 或 project 相同，也属于不同通信 scope。

角色默认是 `peer`；只有显式、非空的用户 `masterGrant` 才能注册 `master`，`auto`
被拒绝。每个 scope 只能有一个 master。`subagent` 必须绑定同 scope 的 parent，
且 parent 在注册时必须仍处于有效 lease。session ID 是地址的一部分，必须由宿主在
注册和 refresh 时稳定提供，不由压缩、fork 或模型名称推断。

agent 注册默认 `working`，默认 lease 为 7 天；`leaseMs` 最低为 1000 毫秒。
`refresh_agent` 只延长该地址的 lease。需要活跃 agent 的发送、注册子 agent、Bug、
Loop 和 adapter 绑定都在操作时检查 lease；过期地址返回 `agent_lease_expired`，
不能被当作仍在线的收件人。

路由规则如下：

1. 同 scope 内，master 可以和该 scope 的 peer 或其 subagent 通信；subagent 只能
   和自己的 parent 或可证明的 master ancestor 通信。
2. 同 scope 的 peer 之间只有在 `appserverId` 和 `projectRoot` 都相同的情况下才
   允许互通。peer 与非自身绑定的 subagent 不互通，subagent 与 subagent 永远不互通。
3. 不同 scope 只允许源 scope 的 live master 发给目标 scope 的 live master；不能把
   目标静默改写成 master，也不能让 peer 或 subagent 跨 scope。
4. 未知、未注册或 lease 过期的地址不能获得 master 权限。角色、scope、parent 或
   route 不明确时 fail-closed。

## 消息、尝试和结果语义

消息必须包含 `from`、`to`、`title`、`priority`（`p0` 到 `p3`）和 `body`。消息事实
先写为 `created`，内部持久化确认后追加 `accepted`；`accepted` 是 AppSDK 的事实
确认，不等价于 adapter 已投递，也不等价于宿主已执行。

`messageId` 是幂等键，但幂等只对完全相同的消息语义成立：

- 重试发现只有 `message.created`，必须补写缺失的 `accepted` state；
- 已有 accepted state 但没有 notification 时，必须按原消息恢复通知；
- direct 或 `p0` 恢复独立通知；idle 恢复原 coalesce bucket，并保留该 bucket
  的第一次截止时间；
- 语义不同的相同 `messageId` 返回 `message_id_conflict`，不创建新事实，也不修改
  原消息；恢复过程只追加缺失事件，不能覆盖或丢弃原始 JSONL。

adapter 或宿主只知道“已尝试”时，receipt 使用 `intent` 或 `accepted`；只有实际
  观察到承载成功才能使用 `delivered`。无法观察的结果保持未确认状态或记录
  `unknown` 观测，并保留错误上下文；不得把未知、超时或只生成 intent 当成
  `delivered`、`executed`、`replied`、`read` 或 `consumed`。

adapter 调用前先追加独立的 `notification.delivery_attempt` 事实（包含 attempt ID、操作、
adapter 和开始时间；批量发送还包含 batch ID）。事件顶层的 `attemptId` 必须与嵌套
attempt 的 ID 相同；`notification.queued` 只记录通知本身，idle flush 不会因为开始一次
投递而重复写 queued。重放要求先看到对应的 queued，再应用 delivery attempt；缺少通知、
attempt 无效或 attempt 的 adapter 与通知记录不一致时 fail-closed。

`notification.emitted` 和 `notification.batch_emitted` 都必须带本次 attempt 的顶层
`attemptId`，重放时必须与待完成 attempt 相同；缺失、错配或在没有对应 attempt 时出现的
终态事实都会 fail-closed。`notification.delivery_failed` 必须记录实际操作；只有适配器调用
已经开始时才带 `attemptId`，此时同样必须与待完成 attempt 相同。适配器解析、绑定等调用前
失败没有 delivery attempt，可以只保留 operation 和错误事实。进程在 attempt 事实之后崩溃时，
重放会把仍带有未完成 attempt 且没有 terminal receipt 的通知投影为 `unknown`；它不能被当成
成功，也不能在相同 `messageId` 的幂等恢复中自动重发。终态事实清除 attempt；失败保留
`pending` 和错误。

## 通知聚合、直达和唤醒

投递方式分为：

- `direct`：每次 `send` 立即调用 adapter，并返回该次通知摘要；direct 通知拥有独立
  notification key。
- `idle`：默认等待 120 秒后由 `flush_notifications` 批量发送；`batched` 是同义
  输入。`p0` 无论请求的 delivery mode 如何都立即打断等待。

idle notification 按“发送地址、接收地址、adapter、coalesce key”合并。一个 bucket
的 projection 只保留最新标题、时间、priority 和 issue；完整正文及每一次更新仍
可从 JSONL 读取。bucket 的 `availableAt` 取第一次进入窗口的截止时间，后续进度
不会无限推迟它。flush 按收件地址和 adapter 分组，再按 priority、createdAt 和
notification ID 排序；发送失败保留 `pending` 和 `lastError`，写入
`notification.delivery_failed`，以后可以在同一事实基础上重试。不存在固定 10 秒
探针。

worker 只有在 `working -> idle` 的状态边沿向 scope master 产生一次幂等 idle 通知；
重复观察 idle 不重复建消息，worker 不参与 master wakeup。没有 live master 时，worker
状态仍然先落盘并返回 `master_not_registered`，master 注册后可用同一状态边沿的
语义恢复通知。

daemon 只对 live、仍为 `idle` 的 master 执行状态驱动 `tick`。master idle 后每 120 秒
最多提醒三次，第三次后将 wake cycle 标记为 `stopped`；master 回到 `working` 时
清零并开启下一轮。tick 不向 worker 发送 idle wakeup，也不因固定轮询间隔制造消息。
每次提醒的 wakeup、message、notification 和 receipt 在同一 `wakeup.reminder` 事实
中重放；adapter 错误保持明确失败和未确认结果。

## Bug 和 Loop

`report_bug` 要求 reporter 属于目标 scope 且仍有效，并要求该 scope 有 live master。
它创建 active Bug，同时建立 `bug-loop-<bugId>`：owner 是 scope master，work 指向
独立 worktree，gate 是项目验证与 review，Bug 按 priority（`p0` 最高）进入 Loop
排序。P0 Bug 立即通知 master；其他 Bug 和进度通知进入 idle 批次。

确定性 Bug Loop ID 已存在时，只有在以下条件全部满足时才允许幂等复用：

- `kind == bug`；
- owner 是当前 scope 的 master；
- trigger、work、gate、state、stop 与该 Bug 的绑定语义一致。

已存在但不匹配返回 `bug_loop_conflict`，不创建 Bug、不改写旧 Loop，旧事实继续可重放。
Bug 只有 scope master 可以 `resolved` 或 `closed`，并且必须提供非空的 `fix`、
`verification` 和 `merge` 证据；证据写入 `resolutionEvidence`，Bug、Loop 和回报
reporter 的通知保持可追溯。

master 任务、Bug、subagent 分派和 peer 任务共用一个 Loop：

```text
Trigger -> Work -> Gate -> State -> Stop
```

每轮执行顺序固定为：

```text
Discover -> HandOff -> Verify -> Persist -> Schedule
```

`create_loop` 默认 `maxIterations=100`，也可以声明 deadline。`advance_loop` 完成时
必须提供能证明通过的 gate 或 verification evidence；`number`、`false`、`null`、空数组、
空对象、空字符串以及 `unknown`、`pending`、`failed`、`failure`、`fail`、`error`、
`invalid`、`timeout`、`blocked`、`unverified`、`not_run` 等未通过结果必须 fail-closed。
对象至少包含 `status`、`result`、`outcome`、`state`、`passed`、`success`、`ok` 或
`verified` 之一；`status`、`result`、`outcome`、`state` 只能使用归一化后的
`passed`、`pass`、`success`、`ok`、`verified` 或 `true`，布尔结果字段
`passed`、`success`、`ok`、`verified` 只能为 `true`。对象中的每个结果字段都必须通过，
并递归检查其嵌套对象与数组；描述性字符串只作为非结果元数据保留。完成证据写入 Loop 的
`completionEvidence`，并随 `loop.updated`
和 replay projection 保留。deadline 优先于 complete；达到 maxIterations 进入
`stopped`。Gate 失败、未知 Loop phase 或显式 `record_error` 会留下带 code、message、
context、at 的错误事实；关联 Loop 进入 `blocked` 或 `stopped`，不会静默重试。

## 稳定集成面

```text
appsdk communication <project> --json '<request>'
appsdk communication capabilities
appsdk comm ...
```

请求操作包括 `register_adapter`、`register_scope`、`register_agent`、`refresh_agent`、
`send`、`set_agent_state`、`tick`、`flush_notifications`、`report_bug`、`update_bug`、
`create_loop`、`advance_loop`、`status` 和 `record_error`。查询只读取 replay
projection；变更返回投影和本次写入的事实 ID。

非法 JSON、未知 operation/event、非法角色或 priority、重复 master、未注册或过期
地址、adapter target/recipient 错误和非法 Loop evidence 都用稳定错误 code 失败，
并保留错误事实。未知外部执行结果不能被重写成成功。

## 最终候选验收

- 从 JSONL 重开后，scope、agent lease、route、message、notification、wakeup、Bug、
  Loop、receipt 和错误 projection 与事实一致；坏 JSONL、未知 event 和占用锁均
  fail-closed。
- 相同 `messageId` 的 crash-prefix 重试覆盖“只有 created”和“created + accepted”，
  并确认 direct/P0 独立通知、idle 原 bucket、pending/emitted projection 和
  `message_id_conflict`。
- completion evidence 覆盖缺失、null、false、空对象、空字符串、缺少显式结果字段、未知或
  失败状态、有效 gate/verification 和重开后仍存在的 `completionEvidence`。
- Bug Loop collision 覆盖无关预存 Loop：返回 `bug_loop_conflict`，不写 Bug，旧 Loop
  保持不变；合法 Bug Loop 才能幂等复用。
- route matrix 覆盖同 scope peer、parent/subagent、master、跨 scope master，以及
  过期 lease 的拒绝；只有 live master tick，worker idle 只产生一次通知。
- direct、idle 120 秒、P0 breakthrough、按 adapter 分组的 batch、失败后 pending
  保留和 master 三次唤醒上限都有正反测试。
- 每次真实 adapter 调用只追加一个 `notification.delivery_attempt`；idle flush 重复执行
  不重复写 `notification.queued`，attempt 与 terminal event 的 `attemptId` 必须一致；
  attempt 后崩溃会得到 `unknown`，不能自动重发或冒充成功。
- `cargo` 定向通信测试、全量测试、release build、JSON Schema 语法检查和实际 CLI
  黑盒入口均绑定同一个最终 commit/tree；codexapp 未被修改，也没有外部通信实现
  编译依赖。
