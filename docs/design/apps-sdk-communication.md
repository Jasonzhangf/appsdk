# AppSDK 内部通信与长程 Loop 设计

## 目标、owner 和边界

AppSDK 自己拥有 `appsdk-comm/v1` 的通信协议、身份与 scope、路由授权、通知投影、
事实记录、错误链和长程 Loop。Desktop、TUI 或其他宿主只通过 JSON 调用面接入；
宿主 appserver 可以展示或执行返回的 intent，但不能替 AppSDK 伪造执行结果。
`/Users/fanzhang/Documents/github/codexapp` 只提供通信语义参考，不是运行时依赖，
也不属于本模块的修改范围。

运行时身份由 AppSDK 的 host registry 单独持有：`~/.appsdk/runtimes.jsonl` 是
append-only 的 `runtime.registered` 事实，记录稳定 `runtimeId`、App Server endpoint、
namespace、项目 cwd、宿主声明的 capability、进程和 fingerprint。
会话压缩或 fork 只改变 conversation/session；宿主继续使用同一个 `runtimeId`，不能复制旧
session token。`send_message_to_thread` 是 appserver adapter 的必要 capability；没有该
声明的 runtime 仍可注册，但不能绑定或接收该 adapter 的发送。
`register_runtime` 对相同身份幂等，对同一 ID 的 endpoint、cwd 或 namespace 变化返回
`runtime_identity_conflict`。scope 和 agent 注册必须引用已经登记且完全匹配的
`runtimeId`；缺失或不匹配在写入项目 mailbox 前失败。runtime registry 与项目事实分离，
但所有权仍属于 AppSDK，默认根目录始终是 `~/.appsdk`。

`mailbox` 和 `appserver` 是同一个通信抽象的承载 adapter，不是两套协议或两份事实：

- `mailbox` 是内置的持久化 adapter。通信事件追加到项目下
  `.appsdk-control/communication/mailbox.jsonl`；adapter 返回 `accepted` 只表示
  AppSDK 已持久化，不表示收件人已读取。
- `appserver` 只向绑定的 endpoint 返回 `intent`，receipt 带
  `hostMustExecute=true`。宿主没有回报执行证据前，状态不能升级为
  `delivered`、`executed` 或 `read`。

appserver adapter 绑定收件人的当前 runtime。target 必须等于最新 runtime endpoint，
并且该 runtime 必须声明
`send_message_to_thread`。`register_adapter` 在写入 `adapter.registered` 前执行一次校验，
每次真实发送或 idle flush 前再读取 registry 校验一次。runtime refresh 后旧 target 返回
`appserver_target_stale`，capability 被撤回返回
`appserver_capability_missing`；这些失败发生在 `message.created` 之前，不产生假消息或假
delivery 事实。

通信 store 的命令 root 必须是存在的绝对 canonical 项目路径。root、`.appsdk-control`、
通信目录、mailbox 和独占 lock 的任一已有路径组件是 symlink 时，store 在创建目录、取得
锁或读取事实前拒绝操作。`register_runtime` 与 `register_scope` 携带的 `projectRoot`
必须逐字匹配这个 canonical root；跨项目值不会写入 host registry 或项目 mailbox，
重放 `scope.registered` 也会重新执行同一绑定校验。拒绝本身仍按统一错误链记录
`error.recorded`，但不会产生 runtime 或 scope 成功事实。

宿主完成真实投递或产生回复后，使用 `record_delivery` 把 receipt 回写到同一项目
JSONL。请求必须带消息 ID、目标 `runtimeId`、状态和非空证据；AppSDK 会核对目标 agent
绑定的 runtime，并只接受 `delivered -> executed -> replied -> read -> consumed` 的单调
推进。重复的相同 receipt 幂等，伪造 runtime、状态回退或未知证据明确失败。这样
`accepted`、adapter `intent`、真实投递、目标执行和消费各自有独立事实，不能用 mailbox
存在或宿主预览代替后续状态。

本模块不启动第二个 daemon，不读取外部 Collab、mailbox CLI 或旧终端状态，不把宿主
的 session、模型名或 endpoint 推断成角色。adapter 失败必须保留原始错误和事实，
不能用 fallback 伪造成功。

## 事实、投影和并发写入

JSONL 是唯一持久化事实源。每行是带 `protocol`、`eventId`、`at`、`kind` 和 `data`
的事件；消息、状态、adapter receipt、通知、唤醒、Bug、Loop 和错误都追加到同一
份记录。读取时从头重放得到 `status` 投影；投影不是第二份可写事实。每个非空物理行都
必须是完整 envelope，空行、坏 JSON、协议不匹配、未知事件和 envelope 字段错误均保留
原始行号并 fail-closed；合法事件从头重放后继续得到同一 projection。

同一个 mailbox 命令生命周期使用独占 `.jsonl.lock`：先取得锁，再重放、校验和追加，
最后同步写入。锁被占用返回 `communication_busy`，不在旧投影上继续写。JSONL 坏行、
协议不匹配或未知事件不跳过，统一以 `journal_corrupt` 或
`journal_unknown_event` 失败。

`agent.rebound`、`message.created`、`message.state`、`notification.queued`、`notification.superseded`、
`notification.delivery_attempt`、`notification.emitted`、`notification.batch_emitted`、
`notification.delivery_failed`、`wakeup.reminder`、`master_wake.updated`、
`master_wake.briefing`、`master_wake.decided`、`bug.*`、`loop.*` 和 `error.recorded` 是可重放事件。
错误处理也必须追加事实；如果
错误事实本身写入失败，返回包含主错误和次级写入错误的错误链。

跨项目目标发现使用同一台主机的 `~/.appsdk/communication.jsonl`。该索引只保存
`scopeId/sessionId -> canonical project root`，不复制角色、lease、parent 或路由状态。
发送端先从索引定位目标项目，再从目标 mailbox 重放 scope/agent 身份并执行原有的
跨-scope master 规则；目标地址未知、冲突、过期或不满足 route policy 时仍然失败。
目标 mailbox 的身份重放采用只读路径，避免发送端持有本地独占锁时形成 A→B→A 的锁
反转；完整 mailbox 仍由目标项目的正常打开路径做全量校验。目标文件缺失、截断、坏行、
未知事件或索引损坏都保留明确错误，不会自动改写地址或把消息复制进另一个项目的 mailbox。

项目 mailbox 与 host 索引之间使用可恢复的两阶段事实顺序：先追加带完整本地记录的
`discovery.pending` 意图，再追加 scope/agent/rebind 的本地事实，最后发布 host 索引并追加
`discovery.reconciled`。任一步失败都会保留原始错误和 pending id；下一次打开该项目 mailbox
时按意图补齐本地事实、重试 host 投影并关闭意图。不会把 host 索引当成提交成功，也不会
要求手工复制、删除或改写任一 JSONL。

## 地址、scope、角色和 lease

通信地址固定为 `scopeId/sessionId`，运行时绑定使用稳定 `runtimeId`。Scope 记录 `appserverId`、宿主声明的
`namespace`（`codex_app` 或 `codex_tui`）、由 cwd 确定的 `projectRoot` 和允许的
`sessionIds`。不同 `scopeId` 即使 appserver 或 project 相同，也属于不同通信 scope。

### sessionID、threadID 与 canonical cwd 的三键恢复契约

App Server 绑定必须同时持久化宿主提供的 `sessionId`、native `threadId` 和
canonical project cwd。三者是不同事实，不能互相代替，也不能从模型名、进程名、
历史 route 或 `runtimeId` 推断：

- `sessionId` 是宿主声明的通信 session 地址；注册、refresh、rebind 和
  daemon replay 都必须原样携带并验证。
- `threadId` 是 App Server 实际承载消息的 native thread；注册前必须通过
  `thread/loaded/list` 和 `thread/read` 验证其存在、已加载且 ID 完全一致。
- identity、runtime binding 和 `RouteResolve` 必须保存三者；route 解析只有
  在 `(sessionId, threadId, canonical cwd)` 都匹配当前 binding 时才成功。
- canonical cwd 必须是项目主树，不是 `playground/` worktree。worktree 只是
  任务执行目录；从 worktree 发起 route 解析、context、master status、注册、
  recovery 或 promotion 必须显式失败，不得按父目录、Git common dir 或历史
  route 回退到主树身份。
- `sessionId` 缺失、为空或与 threadID 不一致时，注册与 rebind 显式失败。
  不得用一个字段合成另一个字段，也不得静默选择最近的 route。
- session 变化但 threadID 保持时，使用同一个稳定 identity、同一个
  `bindingId` 和已经持久化的同一个 `runtimeId` 执行显式 rebind；
  `runtimeId` 只在首次注册时从已验真的 transport 生成一次，后续不能由
  新 session/thread 重新派生。旧 `(sessionId, threadId)` 地址进入只读
  tombstone，不能重新注册、refresh、发送、接收或取得 master 权限。
- threadID 变化时同样使用显式 rebind；旧 thread 不能作为 fallback。
  rebind 成功后旧 binding 变为 stale，新 binding 成为 current，role、
  parent、任务关系、`messageId`、`attemptId` 和历史 receipt 保持不变。
- daemon 重启后从追加事实重放 identity、binding 和 tombstone；重放必须得到
  同一 `(sessionId, threadId, canonical cwd)` current binding，不能依赖进程
  内缓存或 worktree cwd。
- tombstone 的 `reboundTo` 是完整 replacement runtime binding，至少包含同一
  identity/runtime/binding 的新 `(sessionId, threadId)` 和 endpoint
  generation；它只用于只读恢复指引，不能作为 route selector。当前
  `RouteResolve` 的错误线上返回该 replacement 的
  `(sessionId, threadId)` 文本指引；完整 tombstone 是 daemon durable
  state，不是错误 payload 的公开结构化字段。

恢复失败时必须保留精确错误和 durable 事实，并给出可执行恢复指引：

1. 缺 `sessionId`、`threadId` 或 canonical cwd：先让宿主重新提供并验证
   三字段，再对同一 identity 执行一次显式 rebind；不要手改 identity、
   binding 或 route。
2. `sessionId`、`threadId` 或 canonical cwd 不一致：保留旧 binding，显式返回
   `SESSION_THREAD_BINDING_MISMATCH` 或 `ROUTE_RESOLVE_INVALID`；修复宿主绑定
   后从 canonical project root 重新注册，不得覆盖旧 binding 或创建替代
   identity。
3. 旧 session/thread 命中 tombstone：返回
   `SESSION_THREAD_BINDING_STALE`，在错误中给出 tombstone 的
   `reboundTo=(sessionId, threadId)`，再向 current 地址发送或消费；
   不得复活旧地址。
4. host route 已写但 daemon 状态不确定：保留双方 journal，重启受影响
   daemon 后按事件重放；host route replay不一致返回
   `HOST_ROUTE_REPLAY_FAILED`，identity/binding replay不一致返回
   `SESSION_THREAD_BINDING_MIGRATION_REQUIRED` 或明确 journal
   replay错误；禁止手工修 route/binding。

注册事务在 host route 发布前已经写入 worker transport 和默认 direct-message
订阅。若 host route 发布是确定的失败，补偿必须同时恢复该 worker 之前的
transport 和全部 notification subscription，并恢复 runtime binding 与
master grant；首次注册没有前一版本时，必须移除未落地的 worker 和订阅。
不能只恢复 route/binding，否则旧 route 虽然可解析，通知仍会尝试发往失败的新
thread。host route 发布结果不确定时不得猜选旧状态，必须保留双方 journal
并按重放结果收口。

角色默认是 `peer`；只有显式、非空的用户 `masterGrant` 才能注册 `master`，`auto`
被拒绝。每个 scope 只能有一个 master。`subagent` 必须绑定同 scope 的 parent，
且 parent 在注册时必须仍处于有效 lease。session ID 是地址的一部分，必须由宿主在
注册和 refresh 时稳定提供，不由压缩、fork 或模型名称推断。

agent 注册默认 `working`，默认 lease 为 7 天；`leaseMs` 最低为 1000 毫秒。
`refresh_agent` 只延长该地址的 lease。需要活跃 agent 的发送、注册子 agent、Bug、
Loop 和 adapter 绑定都在操作时检查 lease；过期地址返回 `agent_lease_expired`，
不能被当作仍在线的收件人。

会话压缩或 fork 后，宿主必须用 `rebind_agent` 携带旧地址、新地址和同一个
`runtimeId` 完成显式重绑定。AppSDK 会重新校验旧 agent 的 live lease、scope、runtime
和新 session 是否由 scope 声明；`agentId`、role、parent、`masterGrant`、lease 与逻辑
状态从旧地址保留，观察时间和过期时间从重绑定时刻刷新。成功操作只追加一个
`agent.rebound` 事件；旧地址从 active projection 移到只读 tombstone，不能重新注册、
refresh、发送消息、接收消息或取得 master 权限。新地址已占用、runtime 不匹配、scope
变化或旧地址已过期时 fail-closed。若被重绑定的是 scope master，`masterSessionId` 和
其已有 wake projection 一并迁移到新地址；重复应用或篡改 before/after/tombstone 关系
在 JSONL 重放时返回 `journal_corrupt`。

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

每条新消息（包括 daemon 产生的系统和 master wake 消息）在首次确认后都建立一个唯一的
`message.delivery_attempt`，并在 `message.created` 的 `deliveryAttemptRequired` 字段写入
`true`。该事实绑定 `messageId`、`attemptId`、一次性 `nonce`、adapter、目标 runtime、
runtime fingerprint、target 和开始时间；同一消息的幂等重试复用这条 attempt，不能创建第二条。
外部 `record_delivery` 必须同时提交 `attemptId` 和 `nonce`，并且只能匹配已经持久化的 attempt；
缺失、错配或目标 runtime 不一致时 fail-closed，不写入 `message.state`。attempt 的 fingerprint
必须来自已登记的 runtime 观察；runtime 后续刷新不会改写历史 receipt，重放通过 fingerprint
历史记录验证。进程在 attempt 后、receipt 前崩溃时，attempt 保留为未决事实，不能推断为
`delivered` 或自动生成新的 attempt。

升级前的 `message.created` 没有 `deliveryAttemptRequired` 字段，属于明确的 legacy message
格式。其既有外部 receipt 继续按旧合同重放：必须有目标 runtime、已登记的历史 fingerprint
和非空 receipt，且 runtime 必须仍匹配收件 agent；旧事件不能因为缺少新 attempt 字段而阻断
整个 mailbox。只要 legacy message 已有 persisted attempt，或 receipt 出现任一新 attempt
字段，就切换到严格新合同。这个兼容分支只影响历史重放；新的 `record_delivery` 请求始终
要求 persisted attempt、`attemptId` 和 `nonce`，不会因为 legacy history 而放宽。

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
不会无限推迟它。窗口结束后，已 `emitted`、`unknown` 或 `superseded` 的 bucket
不会吞掉下一次更新：下一条不同 `messageId` 的更新开启新的 `generation`，而同一
generation 内仍只保留最新 projection；旧窗口的完整轨迹只留在 JSONL。flush 按
收件地址和 adapter 分组，再按 priority、createdAt 和 notification ID 排序。
`direct` 消息以及 priority 为 `p0` 的通知永远不进入 idle batch；它们只能通过
独立的 direct delivery/retry 路径处理。flush 发现 pending notification 没有对应
的 message fact 时直接 fail-closed。任何适配器发送失败都保留 `pending` 和
`lastError`，写入 `notification.delivery_failed`，以后可以在同一事实基础上重试。
不存在固定 10 秒探针。

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
状态仍然先落盘并返回 `master_not_registered`。master 注册时会扫描同 scope 仍在
lease 内的 idle worker，并用该 worker 的 `lastStateAt` 重建同一条 deterministic
idle signal/message；已有 signal、message 和 notification 会幂等复用，缺失的
prefix 只补写缺失事实。master 重复注册不会重新发送；过期 worker 不会被伪造为
可投递地址，仍保留其状态事实等待重新注册。

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
- communication JSONL 必须以换行结束；重复 `eventId`、EventRecord 顶层未知字段和
  截断的最后一行都 fail-closed，不能部分重放。
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
  保留、terminal bucket 后的新 generation 和 master 三次唤醒上限都有正反测试；
  master working 时的普通 signal 会被 hold，idle 后只产生一条 briefing。master
  延迟注册时已落盘的 worker idle edge 也必须只补偿一次。
- master wake signal 的稳定 key/generation、P0 direct 不重复聚合、superseded
  notification、generation 冲突和 hold/dispatch decision 都有正反测试。
- 每次真实 adapter 调用只追加一个 `notification.delivery_attempt`；idle flush 重复执行
  不重复写 `notification.queued`，attempt 与 terminal event 的 `attemptId` 必须一致；
  attempt 后崩溃会得到 `unknown`，不能自动重发或冒充成功。
- message delivery receipt 覆盖缺失 attempt、错 `attemptId`、错 `nonce`、runtime 刷新后
  的历史 receipt 幂等、attempt 重放篡改和系统 wake 消息的 attempt 持久化；没有对应 attempt
  的 receipt 不会写入 `message.state`。
- `cargo` 定向通信测试、全量测试、release build、JSON Schema 语法检查和实际 CLI
  黑盒入口均绑定同一个最终 commit/tree；codexapp 未被修改，也没有外部通信实现
  编译依赖。
