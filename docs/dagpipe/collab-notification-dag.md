# Collab AppServer 通知与恢复 DAG

## 目标

关闭当前通信 DAG 的 AppServer route 失效开口：旧 thread 已死时，daemon 自己根据
稳定 `session_id` 找回同 session 的 live thread，通过既有 durable registration
路径重新绑定 route，并只重试当前消息一次。

本图是设计真源，不代表当前实现已经接通。实现只能补齐图中缺链，不得让 tmux、
peer、CLI、master 或用户手工恢复成为主路径。

## 基础能力确认

| 能力 | 真源 | 结论 | 使用边界 |
| --- | --- | --- | --- |
| daemon 选择 transport | `WorkerRec.transport` / `SelectedTransport` | 可用 | daemon 是发送前 transport owner；peer 不能手改 route |
| default direct-message lease | `default_direct_message_events` | 可用 | registered peer/master 的 daemon 基础接收随身份保留；不存在公开退订入口 |
| durable mailbox | `Event::Sent` / `msgs` | 可用 | mailbox 只承载 payload，不替代通知 |
| subscription lifecycle | `NotificationSubscribed` / `NotificationSuppressed` | 可用 | 注册 owner 创建或对齐 default lease；default direct-message 由 daemon 唯一拥有，历史 explicit-unsubscribe 不再阻止恢复 |
| AppServer immediate wake | `immediate_notify` | 可用 | success 只代表 AppServer accepted，不代表消费 |
| AppServer queued wake | `queued_notify` | 可用 | 同样只证明 native accepted |
| thread not found 判定 | `AdapterError::RouteUnavailable` | 可用 | 只有 route unavailable/thread missing 可触发 repair |
| live candidate 发现 | Codex `thread/list` | 可用但需新增封装 | `thread/list` 无 sessionId filter；需分页、cwd filter、本地过滤 |
| candidate 可选投递字段 | Codex `Thread.canAcceptDirectInput` | 可用 | 只作为可投递性判断，不能替代 thread identity |
| durable rebind | `RegisterWorker` / `GlobalCurrentThreadRouteSet` | 可用 | 复用既有注册和 current route commit 路径 |
| 消费 receipt | `collab recv` | 可用 | 独立闭环，不从 notification accepted 推断 |

关键限制：AppServer `thread/list` 的 `sessionId` 在返回的 thread 对象内，不是查询参数。
daemon 必须分页读取，按旧 thread 排除、同 `session_id` 过滤、同 cwd 过滤，并要求候选唯一。
多个候选、无候选、旧候选或查询失败都 fail closed，不切 tmux。

## 通知 DAG 与独立 route repair DAG

```mermaid
flowchart LR
  A[收到持久化通知请求] --> B[解析注册 worker 和当前 transport]
  B --> C[恢复 default direct-message lease]
  C --> D[按已绑定订阅尝试 AppServer 通知]
  D --> E[分类通知结果并决定是否 repair]
  E --> R[进入 route repair 图]
  R --> F[按 session_id 查询 live thread]
  F --> G[验证同 session 候选唯一]
  G --> H[通过既有注册路径 durable rebind]
  H --> I[刷新 worker transport 和订阅]
  I --> J[对当前消息只重试一次]
  J --> Z[提交 terminal notification receipt]
```

`appsdk-collab-appserver-route-repair@0.2.0` 是独立 SESE 图：入口为 `route_unavailable_failure`，出口为 `notification_result_receipt`。它只在已经证明的 route unavailable/thread missing 之后进入，不再要求首次成功通知也必须经过 repair。

主通知图的 terminal receipt 只能是：

- `notification_accepted`：原生 AppServer 接受第一次或唯一一次重试；
- `route_repair_required`：无候选、多候选、候选不可投递或 rebind 失败；
- `notification_delivery_failed`：非 route 类失败。

route repair 图内的节点是严格线性闭环；repair 内部的 no-candidate、ambiguous、old-candidate、discovery incomplete、rebind failed、retry failed 都归入同一个 terminal receipt 类型，不能静默回退到 tmux。

## 设计态 compile gate

`docs/dagpipe/manifest.json` 是 AppSDK DAG 设计清单，包含 contracts 里的两张可执行图和 docs 里的 Collab 设计图。`appsdk dagpipe validate` 会对每张图执行 SESE 拓扑检查、operator registry 注册，以及 `compile(graph, &registry, &capabilities)`。这里证明的是图拓扑和 owner/operator 注册边界，不证明 Collab runtime 已实现；route discovery、rebind、lease refresh、retry 和消费 receipt 仍以后续实现验证为准。

## 订阅生命周期 DAG

```mermaid
flowchart LR
  A[worker 注册并选择 transport] --> B[创建或刷新 default direct-message lease]
  B --> C[校验订阅 target/method 与 transport 一致]
  C --> D[提交 active subscription state]
```

单源：`worker_transport_registration`。
单汇：`active_subscription_state`。

daemon 是 default lease 的唯一 owner。注册时或 route rebind 后，订阅必须重新指向
当前 worker transport；订阅与 transport 不一致是发送前 fail-closed 条件，不能静默
变成 mailbox-only。default lease 的历史 `cancelled`（含历史 explicit-unsubscribe）属于
可修复状态：恢复时按当前注册 identity 与 daemon 所选 binding 重新 armed，并绑定尚未尝试、
无 acceptance、无 uncertain outcome 的 pending direct message。有限 schedule occurrence
的 ended 仍为终态，不进入当前 registration-to-active 的 repair 回边。

通知图的 retry 只允许由
`rebind_current_route` 的新 route 触发一次，不能回到通用发送节点无限循环。

## 消费 DAG

```mermaid
flowchart LR
  A[接收方请求消费] --> B[校验 mailbox owner]
  B --> C[读取持久化 payload]
  C --> D[提交 consume receipt]
```

这个图与通知图独立：通知 DAG 不知道消费是否发生，消费 DAG 不等待通知结果，也
不修改订阅、route 或 transport。mailbox 缺失订阅时仍保留完整 payload，但发送结果必须显式
`repair_required`，不能伪装成成功通知。

## 精确目标调度 DAG

每次操作使用目标项目、应用范围、稳定名字和目标 ID 选择对应记录。任务目标的内容和
周期来自任务单。订阅 ID 从对应回执读取，用于核对批次和进度。

```mermaid
flowchart TD
  S([收到目标操作]):::boundary --> A[核对授权和精确目标]:::action
  A -->|权限或目标冲突| N[保留原状态并返回原错]:::failure
  A -->|选择完成| D{操作意图}:::decision
  D -->|查询| Q[读取状态与当前批次]:::action
  D -->|编辑| B{匹配的批次}:::decision
  B -->|唯一有效| E[核对指定批次并更新允许字段]:::action
  B -->|只有终态记录| T[编辑最近的终态记录并保持终态]:::action
  B -->|缺失或歧义| N
  D -->|显式启动或授权续订| R[复用有效批次或登记新批次]:::action
  D -->|到期单次重新登记| U[同一提交中替换到期批次并保留单次形态]:::action
  D -->|结束| C[结束指定计划并读取提交时的实际终态]:::action
  D -->|任务派生| P[随任务提交适用计划并保留终态]:::action
  E --> W[校验并持久提交]:::action
  T --> W
  R --> W
  U --> W
  C --> W
  P --> W
  W -->|拒绝或提交失败| N
  W -->|提交成功或已达目标状态| O([输出统一结果与回执]):::boundary
  Q --> O
  N --> O

  classDef boundary fill:#E9EEF5,stroke:#64748B,color:#0F172A,stroke-width:1.5px
  classDef action fill:#E8F1FF,stroke:#2563EB,color:#0F172A,stroke-width:1.5px
  classDef decision fill:#FFF4CC,stroke:#D97706,color:#0F172A,stroke-width:1.5px
  classDef failure fill:#FDECEC,stroke:#DC2626,color:#0F172A,stroke-width:1.5px
```

图的单入口为 `goal_request`，单出口为 `goal_result`。
一次操作内没有重试回边。客户端结果未知时，先读同一目标的当前回执再决定是否重试。
AppSDK 编辑使用 daemon 订阅回执中的 ID 作为前置条件。若当前批次发生变化，daemon
拒绝该编辑。结束回执带锁内确认的批次及终态，覆盖客户端读取与提交之间发生的到期。
新批次的启动权限沿既有目标合同。任务派发、任务改动及其订阅事件在同一业务提交中发生。
机器图见 [目标归属与调度](collab-goal-ownership.graph.json)。

### 保护角色与 2 分钟批量合流策略（2026-11-27 消融契约）

- 统一策略门在原生传输选择前同时检查接收方角色（current `master` 或 `patrol`/`memory` 专长）与合并窗口。
- `master` / `patrol` / `memory`：普通事件仅持久入箱并保持 `Deferred`（不记失败、不立即调用 native adapter、不重置或加速巡检周期）；仅在既有周期巡检或目标调度到期时随巡检一并投递，且禁止 `turn/steer`（AppServer 使用 `queued`，DSH 使用 `inject`，Desktop/Rex 仅在确认 idle 时投递）。
- 普通接收方：全传输统一适用 `[t0, t0 + 120_000)` 毫秒固定收集窗口，全量合并窗口内合格消息，不因 `MAX_BATCH_DELIVERY` 预览截断而滞留成员；预留与提交均以全批次成员列表为准。`Deferred` 不得触发 `route_repair` 或记录 `NotificationDeliveryFailed`。

## 实施映射

| 语义节点 | owner | 当前状态 | 缺口 |
| --- | --- | --- | --- |
| 解析注册 worker 和当前 transport | Collab daemon | 已实现 | 无 |
| 创建或刷新 default direct-message lease | Collab daemon | 已实现 | route rebind 后必须刷新订阅；历史取消自动迁移，不再依赖显式订阅 |
| 校验订阅与 transport 对齐 | Collab daemon | 已实现 | 不对齐时必须 fail closed，而不是 mailbox-only |
| 尝试 AppServer 通知 | AppServer adapter | 已实现 | 无 |
| 分类 route 失败 | AppServer adapter + daemon | 部分实现 | 只在 `RouteUnavailable` 触发 repair |
| 查询 live session thread | AppServer adapter | 缺失 | 新增 `thread/list` candidate discovery |
| durable rebind | Collab registration owner | 路径可用 | daemon 在 notification failure 后复用 |
| 单次 retry | daemon notification owner | 缺失 | 只在 repair 成功后调用一次 |
| 提交 consume receipt | Collab mailbox owner | 已实现 | 独立，不合并到通知图 |
| 绑定尚未尝试的 pending direct message | Collab daemon | 已实现 | 恢复 default 后追加普通 WakeBound，再由既有 attempt owner 只投递一次 |
| 收敛 owned goal 有限调度 | Collab daemon + AppSDK goal owner | 已实现 | `collab goal reconcile` active/ended，不接受任意 subscription ID，不触碰 default receive |

## 失败终点

- 无同 session live thread：`APPSERVER_ROUTE_REPAIR_NO_CANDIDATE`，`repair_required=true`。
- 多候选：`APPSERVER_ROUTE_REPAIR_AMBIGUOUS`，不得猜测较新 thread。
- 候选仍是旧 thread：`APPSERVER_ROUTE_REPAIR_INVALID_CANDIDATE`。
- `thread/list` 不可用或分页不完整：`APPSERVER_ROUTE_REPAIR_DISCOVERY_FAILED`。
- rebind 失败：保留原失败，并记录 `repair_required=true`。
- retry 后仍失败：保留原失败，不再循环。

## 必测场景

1. 旧 AppServer thread not found，同 session 有唯一 live thread：daemon 自动 rebind，
   notification retry accepted。
2. 同 session 无 live thread：保持 route repair required，不切 tmux。
3. 同 session 多个候选：fail closed，报 ambiguous。
4. 候选仍是旧 thread：拒绝重绑。
5. rebind durable：daemon replay 后 current route 指向新 thread，旧 route 记录保留。
6. notification accepted 但无 recv：consumption 仍缺失；只有 recv receipt 才算消费。
