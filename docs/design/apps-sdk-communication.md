# AppSDK 内部通信与长程 Loop 设计

## 目标、owner 和边界

AppSDK 自己拥有 `appsdk-comm/v1` 的通信协议、身份与 scope、路由授权、通知投影、
事实记录、错误链和长程 Loop。Desktop、TUI 或其他宿主只通过 JSON 调用面接入；
宿主 appserver 可以展示或执行返回的 intent，但不能替 AppSDK 伪造执行结果。
`/Users/fanzhang/Documents/github/codexapp` 只提供通信语义参考，不是运行时依赖，
也不属于本模块的修改范围。

运行时身份由 AppSDK 的 host registry 单独持有：`~/.appsdk/runtimes.jsonl` 是
append-only 的 `runtime.registered` 事实，记录稳定 `runtimeId`、App Server endpoint、
namespace、项目 cwd、可选 tmux session/pane、进程和 fingerprint。会话压缩或 fork 只改变
conversation/session；宿主继续使用同一个 `runtimeId`，不能复制旧 session token。
`register_runtime` 对相同身份幂等，对同一 ID 的 endpoint、cwd 或 namespace 变化返回
`runtime_identity_conflict`。scope 和 agent 注册必须引用已经登记且完全匹配的
`runtimeId`；缺失或不匹配在写入项目 mailbox 前失败。runtime registry 与项目事实分离，
但所有权仍属于 AppSDK，默认根目录始终是 `~/.appsdk`。

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

宿主完成真实投递或产生回复后，使用 `record_delivery` 把 receipt 回写到同一项目
JSONL。请求必须带消息 ID、目标 `runtimeId`、状态和非空证据；AppSDK 会核对目标 agent
绑定的 runtime，并只接受 `delivered -> executed -> replied -> read -> consumed` 的单调
推进。重复的相同 receipt 幂等，伪造 runtime、状态回退或未知证据明确失败。这样
`accepted`、adapter `intent`、真实投递、目标执行和消费各自有独立事实，不能用 mailbox
存在或 tmux 屏幕文本代替后续状态。

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

`message.created`、`message.state`、`notification.queued`、`notification.superseded`、
`notification.delivery_attempt`、`notification.emitted`、`notification.batch_emitted`、
`notification.delivery_failed`、`wakeup.reminder`、`master_wake.updated`、
`master_wake.briefing`、`master_wake.decided`、`bug.*`、`loop.*` 和 `error.recorded` 是可重放事件。
错误处理也必须追加事实；如果
错误事实本身写入失败，返回包含主错误和次级写入错误的错误链。

## 地址、scope、角色和 lease

通信地址固定为 `scopeId/sessionId`，运行时绑定使用稳定 `runtimeId`。Scope 记录 `appserverId`、宿主声明的
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

master wake 是唯一的 master 运行态唤醒 owner。worker idle、普通 Bug、Loop error 和
其他项目更新先按稳定 `signal.key` 写入该 master 的 `MasterWakeAccumulator`；同一 key
和相同内容重放为幂等，内容变化才递增 `generation`。已由 `handled`、`dispatch`、
`complete` 或 `completed` 消费的 signal 会保留在 `consumedSignals`，同一状态边沿不能
再次激活；`hold` 会持久化为 `held`，重复观察 master idle 不会解除 hold。master 为
`working` 时只积累，不会把相同信号单独 flush 给 master；master 转为 `idle` 后，daemon
的 `tick` 在首个信号进入窗口后的 120 秒生成一条有界 briefing。P0 signal 走同一
direct delivery/receipt 链，不能只因为 priority 高就标成已送达。briefing 包含 generation、
P0/P1 优先级、idle worker、active Bug 和 active Loop 摘要，并将被覆盖的普通 pending
notification 写成 `notification.superseded`，因此同一更新不会同时以普通通知和 wake briefing
打扰 master。被接管的 notification 在 briefing terminal decision 前始终由 accumulator
持有，即使 master 已经 idle；delivery 为 `unknown` 时不会自动重发或改渲染成另一条 briefing。

briefing 的 message、notification 和 attempt 使用稳定的 generation/reminder 身份。恢复时
如果该身份的 message 已写入，直接复用 JSONL 中原始 message body，不按当前 active Loop
或 Bug 列表重新渲染。attempt 之后进程崩溃时，重放将 notification 标为 `unknown`，同一
generation 不会重新投递或消耗提醒次数；只有看到明确的 terminal receipt 才能继续。master 通过
`master_wake_decide` 携带精确 generation 写入 `hold`、`dispatch`、`handled`、
`complete`、`completed` 或 `schedule`。generation 不匹配直接失败；delivery/ACK
不能自动清除信号，只有显式调度类 decision 才能清空 accumulator。每个 generation
最多提醒三次，第三次标记 `stopped`，避免 idle 堆积；对仍有 active signal 的显式
`schedule` 会保留这些 signal、重置提醒预算并开启新的 generation，确保消息身份和
投递预算都是新一轮；没有 active signal 时 schedule 只保持空闲状态。

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

请求操作包括 `register_runtime`、`register_adapter`、`register_scope`、`register_agent`、`refresh_agent`、
`send`、`record_delivery`、`set_agent_state`、`tick`、`flush_notifications`、`report_bug`、`update_bug`、
`create_loop`、`advance_loop`、`accumulate_wake`/`record_wake`、`master_wake_decide`、
`status` 和 `record_error`。查询只读取 replay
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
  保留和 master 三次唤醒上限都有正反测试；master working 时的普通 signal 会被
  hold，idle 后只产生一条 briefing。
- master wake signal 的稳定 key/generation、P0 direct 不重复聚合、superseded
  notification、generation 冲突和 hold/dispatch decision 都有正反测试。
- 每次真实 adapter 调用只追加一个 `notification.delivery_attempt`；idle flush 重复执行
  不重复写 `notification.queued`，attempt 与 terminal event 的 `attemptId` 必须一致；
  attempt 后崩溃会得到 `unknown`，不能自动重发或冒充成功。
- `cargo` 定向通信测试、全量测试、release build、JSON Schema 语法检查和实际 CLI
  黑盒入口均绑定同一个最终 commit/tree；codexapp 未被修改，也没有外部通信实现
  编译依赖。
