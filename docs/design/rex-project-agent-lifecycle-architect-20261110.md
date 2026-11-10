# Rex 完整替换 tmux+iTerm2：架构闭环 2026-11-10

## 1. 范围与结论

这份文档闭合 Astra 六项 P1、reviewer 指出的名字池注册阻塞和 endpointless
Desktop 旧绑定退役缺口。它只定义实现合同、图/gate 边界和验收输入，不证明
源码、安装或真实前端已经通过。

本稿只承载独立 plan r2 的目标合同。图、表格、fixture、JSON 校验或本稿
都不是实现证明；未取得真实入口回执前，所有新 gate 均标为 required /
unverified。

最终目标不变：真实 Rex agent 在普通 `collab context` 下自动注册。另一个
真实参与者能通过 Collab 发送消息。目标 agent 被通知后自主正式 `recv`、执行任务
并发送关联回复。发送方正式消费回复。tmux、Desktop 和 DSH 的受影响路径继续兼容。

## 2. 真源与输入

- 权威需求：`/Users/fanzhang/Documents/github/appsdk/.appsdk/requirements.json`
  version 10，重点约束为 `req-minimal-governance`、`req-collab-identity`、
  `req-command-logs`、`req-minimal-command-surface`、
  `req-collab-communication-notifications` 和
  `req-collab-daemon-restart-recovery`。
- Jenny 执行交接：
  `/Users/fanzhang/.agents/task-notes/rex-api-foundations-20261108/parity-20261109/jenny-execution-handoff.md`。
- Astra 设计审查：
  `/Users/fanzhang/.agents/task-notes/rex-api-foundations-20261108/parity-20261109/astra-design-review.md`。
- 真实自动注册红测：
  `/Users/fanzhang/.agents/task-notes/rex-api-foundations-20261108/parity-20261109/receiver-registration.md`。
- 已记录 Rex 样本（本轮未重跑，不是当前状态证明）：官方 endpoint 为
  `unix:///Users/fanzhang/Library/Application%20Support/rex/server.sock`。
  该样本的 agent sessionID 为
  `01a1c473-41b6-7481-82d4-e2499ac2490a`，Rex session 为
  `session:01a1baae-5766-7724-92d9-59a846daaf02`，Rex block 为
  `block:01a1c455-1e66-73b5-a116-3f10212ab620`。
- 已记录失败样本：安装版 `collab 0.2.0457` 的普通 `collab context` 曾选择
  `desktop_bridge`。带真实 Rex facts 和 `--work-roles architect` 的一次
  `--provide` 返回 `DESKTOP_BRIDGE_THREAD_REQUIRED: current native thread ID is
  missing`。该结果证明未选用字段仍会阻断 Rex 路径。它不等于新候选失败。

## 3. 唯一 owner 与并发边界

| 对象 | 唯一 owner | 说明 |
|---|---|---|
| 当前身份、名字和业务绑定 | 现有 `identity-map.json` 与 identity gate | 不新增身份表，不从日志、Rex 标签或 pane 猜身份 |
| agent sessionID | 当前宿主事实 | Rex session/block 不得覆盖或混合 |
| Rex server/session/block/cwd | 现有 Rex typed transport binding | 四个字段只定位终端，不承担工作归属 |
| 当前 runtime anchor | 现有 binding owner | 每个 worker 同一时刻只有一个当前 anchor |
| 任务世代与未关闭 assignment | board 与 task owner | 名字续接只更新 carrier，不重建任务 |
| 未消费消息与回复义务 | mailbox owner | 名字归属变化时跟名字迁移；已消费不重放 |
| 项目 workspace 与布局 | 现有 layout manifest owner | Rex session/window/block 只扩展布局映射 |
| daemon 维护 | 用户 | 实现、测试和验证不关闭或重启生产 daemon |

工作树只有一名 writer。当前 Kate 任务持有
`/Volumes/Intel/playground/appsdk/rex-tmux-parity-20261109`。等待其交付时，其他
任务不得并发写同一事实。独立架构或计划任务使用自己的 Git worktree。

## 4. P1-1：主体匹配与单 anchor

每个提交以 `(canonical project, app scope, agent sessionID, anchor)` 为一个
identity gate 事务。名字标签参与显示和布局，不补主体连续性。

| 输入组合 | 裁决 | 失败终点 |
|---|---|---|
| 同一 active slot，新 agent sessionID，anchor 不变 | 保留名字和身份，受控换绑 session | 成功；未关闭 assignment 同步更新 |
| 同一 active slot，agent sessionID 不变，anchor 变化 | 保留名字和身份，受控替换当前 anchor | 旧 anchor generation 失效 |
| session 与 anchor 都变化，但用户授权和当前权威事实唯一命中一个 active slot | 一次受控换绑；cwd 只选择项目或提供线索，单独不证明身份连续 | 不唯一时返回明确歧义 |
| session 命中 A，anchor 命中 B | 拒绝本次提交 | 保留现有 `IDENTITY_MAP_AMBIGUOUS` / `RUNTIME_BINDING_REJECTED`；双方不变 |
| 跨 adaptor 换绑，当前 host fact 唯一 | 一次替换 anchor 类型 | 冲突时显式拒绝 |
| 同一 anchor 并发提交 | identity gate 串行化，后到者核对 generation | 旧 generation 拒绝，不改当前状态 |
| 无当前 anchor 的新主体 | 正常新注册 | 使用可用名字或注册编号 |

同一 worker 的当前 anchor 在提交成功后必须唯一。换绑不能在提交前清空旧绑定。
提交失败保留旧绑定。提交成功后未知保留新事实和 pending effect。

## 5. P1-2：15 分钟观察竞争

观察键固定为 `(attempt_id, execution_generation, anchor_kind, anchor_id)`。
它不是 worker 全局计时，也不是每轮查询计时。

15 分钟分支只适用于已派发 attempt 的持续“无感知且无响应”。它不是通用
timeout，也不适用普通 idle、busy、unknown 或单次查询失败。观察到真实状态、
收到回执或获得用户授权时立即退出该分支。accepted/unknown attempt 不因
15 分钟到期自动变成安全可重投工作。

计时从派发提交后首次判定"无法感知且无响应"开始。以下事实立即结束该无信号分支：

- tmux/Rex snapshot 或事件报告 live。
- agent activity 报告 idle、busy、working 或 blocked。
- 收到真实派发接受或响应。

轮询不重置计时。缓存 snapshot 必须带观测时间。只有同一观察键的当前事实能重置
该键。daemon 重启恢复同一观察键和开始时间，不重新计时。anchor 换绑使旧键失效。
到期提交前重读当前 binding 和最新事实；此时可感知或有响应则保留。迟到事件按原
generation 记录，不能写回新 anchor。

以下事实必须分开保存：

- terminal exists
- agent activity
- dispatch accepted
- message received
- message consumed
- reply sent

任何一项都不能替代另一项。

## 6. P1-3：stale 退役对象与业务责任

Jason 已决定退役"当前执行关系和 anchor"，保留 peer 身份与名字。

| 对象 | stale 结果 | owner |
|---|---|---|
| 当前 anchor | 标记 stale 并解除当前执行关系 | binding owner |
| 派发 attempt | 保留 accepted 或 unknown 原事实，关闭本次观察 | dispatch owner |
| execution generation | 停止接受旧 start/result | task owner |
| peer 身份和名字 | 保留 | identity/name owner |
| grant | 保留原事实；新载体不继承旧 master grant | authority owner |
| 未关闭任务 | 保留 ID、状态、证据和资源 | board owner |
| 未消费消息和回复义务 | 随名字保留，不重放已消费消息 | mailbox owner |
| 迟到回执 | 按原 attempt/generation 保存，不作用于新 anchor | dispatch/task owner |

普通下一次 `collab context` 可用同一名字补齐新 anchor。恢复不要求手工
`--provide`，后台维护也不得伪造消费或把 unknown 写成确定执行。

timed stale 只退役当前执行关系与 anchor，不自动重投已接受或 unknown 的工作。
新工作必须由 owner 显式派发；旧 attempt 事实保留。

## 7. P1-4：名字冲突、双主体和名字池不阻塞注册

### 7.1 冲突裁决

| 当前名字状态 | 原 peer 状态 | 用户仲裁 | 结果 |
|---|---|---|---|
| 空闲 | 无 | 无 | 正常分配 |
| 同一工作者续接 | idle 或 busy | 无 | 保留名字，换绑 carrier |
| 其他当前主体 | live+idle | 未仲裁 | 保留原 peer；申请者取其他名字 |
| 其他当前主体 | live+busy | 未仲裁 | 保留原 peer；申请者取其他名字 |
| 其他当前主体 | idle 或 busy | 明确顶替 | 原 peer 获得新名字；名字交给新主体 |
| 状态不可判断 | unknown | 未仲裁 | 最多观察 15 分钟，不凭空空判 idle |

顶替时必须分别处理名字、任务世代、消息、唤醒和授权：

- 名字、未关闭任务和未消费消息按现有 handoff owner 转移。
- credential 与 master grant 不随名字继承。
- 旧 generation 的 claimed/running 停止接受旧结果。
- 新载体重新排队尚未执行的工作。
- 原 peer 的真实活动和名字变化作为持久通知交给项目 master。

### 7.2 默认名字池耗尽

默认名字池只有 20 个名字。池满不得阻塞注册。identity owner 必须持久化
project/app scope 内单调递增的 `registration_number`，每个新主体得到唯一编号。
该 registration-number / 自由名是本计划的目标设计，尚未由当前实现或图验证；
当前红测仍以 `NAME_POOL_FULL` 为失败终点。
分配顺序如下：

1. 复用该主体当前名字。
2. 使用空闲默认名字。
3. 默认池满时使用稳定自由名 `peer-<registration_number>`。
4. 用户明确顶替时，被顶替 peer 先取空闲默认名字；仍无空位则取下一个
   `peer-<registration_number>`。

自由名与默认名字具有相同工作归属、任务、mailbox 和恢复语义。名字变更不得产生
第二身份。注册回执必须返回实际名字和 `registration_number`。重复 context 返回
同一结果，不重复分配编号。达到名字池上限不是失败终点。

## 8. P1-5：标签准备、部分失败和待办

session/block 标签准备是同一布局事务：

1. 读取并保存 session 和 block 的精确旧标签。
2. 记录准备阶段实际完成的每一步。
3. 任一标签失败时，回滚本次已改标签。
4. 回滚失败时返回 partial，并报告实际旧值、当前值和未收口项。
5. 只有全部准备完成才发布身份 binding。
6. 身份发布后的任务、消息、唤醒和报告效果由既有 daemon owner 继续处理。

身份已经发布但效果未完成时返回 pending 或 partial。发送通知不等于对方消费。
不得要求 agent 手工重新 `context` 才完成 daemon 后台效果。

## 9. P1-6：项目 workspace 与 Rex 布局

| 对象 | 合同 |
|---|---|
| machine | 每台机器独立 Collab daemon、身份、任务、mailbox 和 Rex server |
| project | canonical cwd 选择项目；每台机器每项目一个对应 Rex session |
| Rex session | 工作区容器；名称与项目名对齐；保存精确 session ID |
| Rex window | tab；用于展示和分组，不选择身份 |
| Rex block | pane；每个 agent 一个 block；保存精确 block ID |
| slot | layout owner 持有固定 slot、window 和 block 顺序 |
| profile | 先取得 Rex 等价能力证据；无等价项由用户决定，不静默丢弃 |
| create | 布局 gate 序列化；重复项目记录显式歧义，不重复启动 agent |
| rename | 项目改名同步 session 名；共享 session 时只改自己的 window/block 标签 |
| launch/resume | 幂等；已有同 agent 复用；busy 或 waiting_user 保留现场 |
| cross-machine | 使用既有 `machine.project.name` 和 federation owner |

`machine.session.name` 必须按现有 `machine.project.name` 的通信语义解释。
省略 machine 时继续兼容本机地址。Rex 标签不作为地址选择 truth。

可实测提升使用同一任务对照 tmux+iTerm2 与 Rex。对照项为项目是否只有一个 workspace、
agent 是否各有稳定 block、重启后是否恢复、消息是否真实消费和回复、跨机器是否双向消费。
每项只报告 pass/fail、实际 manual steps、证据路径和剩余缺口，不虚构百分比。

## 10. endpointless Desktop 旧绑定

`appsdk.Olivia` 已记录案例是 `desktop_bridge`、旧 session
`01a1c473-41b6-7481-82d4-e2499ac2490a`、`endpoint_live=null`、presence `cold`、
thread `notLoaded`，`collab worker close` 返回
`PEER_LIFECYCLE_TARGET_UNPROVEN`。该结果证明 close 不能作为恢复路径。

正确处理如下：

1. lifecycle owner 读取当前 binding 和 generation。
2. 当前 thread route 存在且与 binding 一致时，走现有 close/probe 语义。
3. 无 thread route 且 endpoint_live 为 null/cold/notLoaded 时，必须取得 Desktop
   发布者私有 typed runtime descriptor 或 native `read_thread` 对 exact
   session/thread 缺席的证明。
4. 证明缺席时才退役旧执行关系和 anchor；保留 Olivia 的身份、名字、任务、消息、
   凭据和 master 报告义务。
5. 无法证明缺席，包括服务不可达、超时、畸形回执或歧义时，不退役。
   `PEER_LIFECYCLE_TARGET_UNPROVEN` 保持原错；不把它改成成功 close。
6. 同一 active slot 的新载体执行普通 `collab context` 时，只在有权核实的唯一当前
   事实成立后换绑。
7. 后续普通 context 使用 Olivia 或明确授权的新名字绑定当前 Rex carrier。
8. 旧迟到回执按旧 generation 保存，不覆盖新 carrier。

timed stale 是另一个分支，只适用于已派发 attempt 的持续无感知且无响应，不用于
补足 endpointless close 的缺席证明。endpoint_live=null 本身不等于 stale，也不等于
缺席。

公开反例至少覆盖 `endpoint_live=null`、thread 未加载、close unproven、15 分钟前后和
同一名字的新 carrier 注册。合法路径必须保留身份和责任，违规路径必须拒绝伪造 close。

本合同不新增 endpoint generation counter。绑定与 stale 判定只读取现有
execution/name generation 和 exact current binding；不能用新的 endpoint 世代
替代身份、名字或任务事实。

## 11. 图与 gate 更新

本轮更新以下设计图的实际 description 边界：

- `collab-identity-anchor`：单 anchor、多主体冲突和 generation 拒绝。
- `collab-context`：普通 Rex 环境发现、显式 rex facts 优先、未选字段不阻断。
- `collab-name-ownership`：默认池满、`peer-<registration_number>` 自由名和顶替转移。
- `collab-binding-lifecycle`：Rex 端点刷新、旧 anchor 退役和身份保留。
- `collab-stale-identity-maintenance`：15 分钟观察键、重启延续和旧 generation。
- `collab-peer-lifecycle`：endpointless Desktop 的 unproven 结果和 stale 退役边界。
- `collab-rex-adaptor`：普通环境候选、固定官方 CLI、字节排队不等于消费。
- `collab-notification-consumption`：接受、收到、消费和关联回复分阶段。

`collab/docs/verification-map.md` 增加对应公开 gate 与真实 agent 验收。

`--provide` Rex facts 是显式输入，覆盖环境发现；ordinary context 才负责默认
发现。宿主选定后，只校验所选 transport 的必要字段，不得让未选 Desktop 字段
阻断 Rex。上述 gate 列表是 required gate；当前除既有红测和结构校验外，未由
公开绿测或真实 agent 证明。

## 12. 红绿验收

### 12.1 公开入口红绿

| 场景 | RED | GREEN |
|---|---|---|
| Rex 普通环境 | 无 `--provide` 时 `missing_facts` 或 desktop_bridge | `registered=true`、`transport.kind=rex` |
| 显式 rex facts | 被环境发现覆盖 | 显式 facts 为权威候选 |
| Rex 环境缺一项 | 猜地址或落 desktop_bridge | 明确缺项，无半绑定 |
| 错误 server/session/block | 静默 fallback | typed admission 拒绝 |
| 默认名字池满 | `NAME_POOL_FULL` | 新注册成功并返回 `peer-<n>` |
| 名字顶替且池满 | 丢任务或继承 grant | 原 peer 获得自由名，任务和 grant 边界正确 |
| 15 分钟边界 | 每轮查询重置或提前 stale | 同一观察键、边界正确、重启延续 |
| endpointless Desktop | close unproven 被当作失败停止 | stale 只退役执行关系，名字可续接 |
| 已消费/未消费消息 | 重放或丢失 | 已消费保留历史，未消费随名字保留 |

### 12.2 真实 agent 黑盒

1. 在真实 Rex LLM agent 中运行默认 `collab context`，不传 `--provide`。
2. 另一个真实参与者发送带唯一内容的消息。
3. 目标 agent 被通知后自主运行正式 `recv`。
4. 目标 agent 执行请求并发送 `--in-reply-to` 关联回复。
5. 发送方正式消费回复。
6. 保留 Rex 前端、目标 binding、文本、各阶段消息回执和消费证据。

脚本 receiver、外部 CLI 消费、手工 `--provide`、终端 echo、构建和 fixture 均不能
替代上述真实 LLM agent 路径。

## 13. 合并顺序

1. 当前 Kate adaptor 修复完成并交回候选和证据。
2. 本架构树合并最新 `origin/main` 和该候选。
3. 补齐名字池、stale endpointless 和 15 分钟合同对应红绿测试。
4. 运行受影响公开入口、DAG 和适用回归。
5. 真实 Rex LLM agent 端到端通过。
6. 作者自检后做一次独立整体 review。
7. 按交付合同 merge、push 并核对远端。
8. 安装、daemon 激活和跨机 live 按用户逐次批准单独执行。

任何步骤失败都保留原错和未完成状态。不得把分支、候选、CI 或 review 结果写成
生产已激活。
