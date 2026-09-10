# AppSDK 内部通信与长程 Loop 设计

## 目标和边界

AppSDK 提供一个自有的 `appsdk-comm/v1` 控制模块，给 Desktop、TUI 或其他宿主
通过稳定的 JSON 调用面集成。宿主只负责把调用展示在自己的 appserver；通信事实、
授权、路由、通知投影和长程 Loop 状态由 AppSDK 持久化。codexapp 只作为协议语义
参考，不是运行时依赖，也不属于本模块的修改范围。

本模块不创建第二个 daemon、不读取 tmux/mailbox/Collab 状态、不推断宿主身份，也
不把“队列已接受”伪装成目标已经执行。运行态唯一真源是项目下被忽略的
`.appsdk-control/communication/mailbox.jsonl`；每一条 JSONL 都保留原始事件，读取
时重放出当前投影。命令生命周期对同一 mailbox 使用独占锁，先锁定、再重放、再
校验和追加；忙时返回 `communication_busy`，不在旧投影上继续写入。

## 地址、scope 和角色

地址必须是 `scopeId/sessionId` 两段。Scope 同时记录：

- `appserverId`：同一个 App Server 的通信边界；
- `projectRoot`：由宿主的 cwd 解析出的项目边界；
- `namespace`：显式的 `codex_app` 或 `codex_tui`，不从模型名或 endpoint 推断。

注册角色默认是 `peer`。只有请求显式携带非空 `masterGrant`（用户授权文本或其
引用）才能注册 `master`；`auto` 是拒绝值。一个 scope 只能有一个 live master。
Subagent 必须绑定 `parent`，且路由只允许它与 parent 或可证明的 master 祖先通信。

路由规则：

1. 不同 scope 之间只能由双方各自的 scope master 通信；目标不能被静默改写成
   master。
2. 同 appserver、同 project 的 peer 可以互通；跨 appserver 或跨 project 的
   peer 被拒绝。
3. Peer 只能访问自己绑定的 subagent；subagent 之间不互通。
4. 同 scope 的 master 可以和该 scope 的 peer/subagent 通信。不同 master 没有
   从属关系，跨项目工作通过 Bug/Loop 事实协同。

## 消息和事实

消息强制包含 `title`、`priority`（`p0` 到 `p3`）、`body`、发送方和目标地址。
写入后状态先是 `created`，持久化成功才进入 `accepted`；模块没有宿主 appserver
执行证据时不继续声称 `delivered` 或 `executed`。每个状态变化和错误都写入同一
份 JSONL。

发送通道分为：

- `direct`：立即返回一个展示摘要，保留完整消息事实；
- `idle`：进入通知投影，默认等待 120 秒批量发送。`p0` 自动打断等待。

每条消息先写入 `notification.queued`，再由消息指定的 adapter 承载。tmux/appserver
adapter 可绑定一个已注册的收件地址，投递时会再次核对，不能把消息送到无关 pane
或 endpoint。adapter 返回的
`TransportReceipt` 会写入 `notification.emitted` 或 `notification.batch_emitted` 的
原始事件；`intent` 只表示宿主应执行，不能升级为 `delivered`。承载失败会写入
`notification.delivery_failed`，通知保持 `pending` 并带有 `lastError`，后续可以在
不丢事实的前提下重试。

闲时通知按“发送实体、接收实体、adapter、coalesce key”合并，只展示最新标题、时间、
优先级和 issue；完整正文始终可从 JSONL 事实读取。每个 bucket 保留第一次进入窗口
的截止时间，只更新最新摘要；持续进度上报不会把 120 秒窗口无限向后推迟。direct/P0
的每条通知保留独立的可重试投影，失败不会被后续消息覆盖。批量发送是显式 `flush`，因此
宿主可以用自己的调度器，不会因为一个固定的 10 秒探针制造压力。

Worker 从 `working` 进入 `idle` 时最多产生一次幂等通知到 scope master；重复的
`idle` 观测不会产生新消息。没有 master 时状态仍然落盘，同时返回明确的
`master_not_registered` 错误。

Daemon 只对 master 做主动唤醒：检测到 master 仍为 `idle` 后，按 120 秒间隔最多
提醒三次；三次以后将 wake cycle 标记为 `stopped`，直到 master 回到 `working`
才重置。唤醒是状态驱动的 `tick`，不是固定频率向所有 agent 发送 keepalive。
每次提醒以单个 `wakeup.reminder` 事件原子写入 wakeup、message、notification 和
adapter receipt，避免只写入其中一部分。

## Bug 和 Loop

`report_bug` 会创建 active Bug，并把它投影为一个按优先级排序的长程 Loop；没有
scope master 时拒绝新 Bug，避免把 reporter 静默当成 owner。P0 Bug 的通知立即打断，
其他 Bug 和进度通知进入同一批次。Bug 只能由 scope master 用包含 `fix`、
`verification`、`merge` 的事实关闭；Bug 与对应 Loop 在同一事件中完成，完成通知发回
reporter。

每个 master 任务、Bug 处理、subagent 分派和 peer 任务都使用同一个 Loop 结构：

```text
Trigger -> Work -> Gate -> State -> Stop
```

一次 Loop 的执行阶段固定为：

```text
Discover -> HandOff -> Verify -> Persist -> Schedule
```

`maxIterations` 和可选 deadline 是硬停止条件；`complete` 必须携带 gate/verification
事实，deadline 优先于完成请求。Gate 失败、未知错误或 deadline 到期会留下带 code、
message 和 context 的错误事件，并把 Loop 置为 `blocked` 或 `stopped`，不会静默重试。

## 稳定集成面

CLI 使用以下形式，便于 Desktop 直接集成而不复制实现：

```text
appsdk communication <project> --json '<request>'
appsdk communication capabilities
```

JSON 请求的 `op` 包含 `register_scope`、`register_agent`、`send`、
`set_agent_state`、`tick`、`flush_notifications`、`report_bug`、`update_bug`、
`create_loop`、`advance_loop`、`status` 和 `record_error`。查询返回当前投影；
变更返回投影和本次操作的事实 id。非法角色、跨 scope 路由、重复主控、损坏的
JSONL 和未知操作均使用稳定错误 code 失败。

## 验收

- JSONL 可在进程重启后重放，坏行不会被跳过；
- `masterGrant` 缺失、`auto`、重复 master 和未注册 session 均 fail-closed；
- route matrix 覆盖同 scope peer、绑定 subagent、跨 scope master；
- direct 即时、idle 120 秒批量、p0 打断、重复通知合并为最后状态；
- worker idle 单次通知、master 三次唤醒上限和 working 重置均有测试；
- active Bug 出现在优先级排序的 Loop 中，Bug 状态变化可追溯；
- Loop 阶段顺序、硬上限和未知错误均有正反测试；
- codexapp 目录和任何外部通信实现没有修改或编译依赖。
