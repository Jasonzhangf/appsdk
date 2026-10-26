---
name: collab
description: "仅约束已成功注册 Collab 的 peer；master 权限需另行授权。未注册、注册失败或无法注册时独立完成，不适用协作协议。用户明确要求注册时才初始化。"
---

# Collab

本入口保留日常协作规则。命令细节按下表读取 [操作手册](operations.md) 的对应章节，
不整篇加载；手册与本入口属于同一个 Skill。

## 适用身份

只有在当前项目成功注册的 peer 才执行本协议；master 还须有当前项目的明确授权。
加载 Skill、多个 worker 同时运行、看到历史名字或收到普通派单，都不产生注册或
master 权限。只有用户要求注册时才初始化；未注册或初始化失败时保留原错，独立完成
可做任务，不寻找或等待 master。

已注册身份、角色、任务、消息和订阅以当前 `collab context` 为真源。项目由本次真实
cwd 确定，稳定名字承接工作；sessionID 承载当前执行，tmux pane、thread 和 socket
仅定位通信。不得猜身份、复制 token、编辑控制状态或以 pane 内容证明消息消费。
恢复失败只停受影响协作，保留原任务及资源。

## 日常通信

| 任务 | 入口与边界 |
|---|---|
| 恢复当前已注册身份和状态 | 在本项目实际环境运行 `collab context`；仅给当前宿主的真实 runtime 信息 |
| 本项目内向明确接收者发送已授权消息 | `collab sendmessage --to '<真实名字或from>' --subject '<主题>' '<正文>'`；已有接收者时直接发送，不先轮询对方是否空闲，不把跨项目地址传给 sendmessage |
| 向另一项目交付资料或消息 | 先读 [操作手册](operations.md#cross-project-master-communication) 的跨项目章节；源项目当前 master 用 `master send`，目标以 `collab master status` 取当前 `master.worker_id`，普通 peer 交本项目 master 处理 |
| 收件与消费 | 非 DSH 用 `collab recv`；DSH gateway 独占 Poll/Ack，直接处理原生完整正文，不运行 recv/ack |
| 只读查消息 | `collab msg <真实消息ID>` 或 `collab inbox`；只读查询不等于消费 |

收到明确请求后处理并按实际回传通道回复，再接续本人未完任务。发送、入队、消费、
回复和完成分别判断；不因超时盲重发，不发无请求的常规心跳、ACK 或进度消息。
一次性 `codex exec` worker 使用派单约定的 stdout/结果文件，不为回报注册 Collab。

宿主 `list_agents` 只枚举该宿主的子 agent，不是 Collab 项目成员目录。列表里没有
某个名字（例如 Jenny）不能证明目标 master 不存在或没有通信通道；当前会话未注册、
缺 master 授权和目标不可达是不同失败，分别以真实 Collab 回执判断。确实要交付资料
给另一项目时，按上表“向另一项目交付资料或消息”走跨项目章节，不因宿主列表无匹配
就默认要求用户人工转发，也不自行注册、猜身份或提升 master。

## Task routing and role responsibilities

master 管项目目标，peer 管已接受的子目标，两者均负责拆分、解阻、验收和资源收口。
长程目标经任务板邀请可用且同意接收的 peer；一次性执行或观察沿
[统一角色路由](../codex-orchestrator/SKILL.md#codex-路由)。
无可用 peer 时保留长程目标并拆分 worker，不等待注册。

派单使用 [Worker 合同](../codex-orchestrator/references/worker-contract.md#任务合同模板)，
明确允许/禁止、验收、升级与停止点；peer 另带任务 ID、稳定双方身份、目标层级和
集成分工。范围内投入和预算边界不另设一套。Planner/Reviewer 模型、并发调度、接管
与 Review 准入均沿 [codex-orchestrator](../codex-orchestrator/SKILL.md)；任务板记录
本身不授权范围外 backlog 或生产动作。

设计、规划与审查角色按
[文件交接合同](operations.md#设计规划与审查角色的文件交接合同)产出并交接落盘文件，
不直接执行实现。

## Register the long-horizon goal before dispatch

已注册 master/peer 接受长程目标时，各自先恢复或登记本人 goal，再拆解派发。已有目标
直接续接，不重复创建；任务板、笔记和口头承诺不替代 goal 回执。使用当前已支持的
`appsdk goal` 入口，详见
[goal 登记与关闭](operations.md#register-the-long-horizon-goal-before-dispatch)。
master 不代持或代登记 peer goal，旧 runtime 拒绝 peer 时记录具体缺口，不提升身份
绕过。

## Registered patrol duties

项目 Jenny/master 默认承担本项目 worker 巡检。初始化或恢复本人 longhorizon 后，
须确保本人常驻项目巡检已 armed；它是一条 `master-idle` 订阅，subject
`project-patrol`、interval `900000` 毫秒，ensure/renew 复用同一订阅而不并排新建。
有限 goal/task 定时器按生命周期关闭，但单个 goal 结束不关闭这条常驻巡检义务。
每次提醒检查 worker 的 busy/idle、board 任务和按任务取消状态。当前 master 用
`collab notify status` 的 `project_board_patrol` 只读视图检查 peer 取消；该读取不授予
替 peer 恢复或覆盖取消的权限。peer 持有未关闭
board 任务且正态 idle 时应唤醒本人，由本人继续推理并判断下一步；busy 跳过，不
补发；unknown、offline、cold 或 waiting_user 不当作 idle。支持实时状态事件时及时
触发；不支持时默认每 15 分钟检查（当前 `notify status` 报告 realtime 未接通）。

peer 可取消本人 board 巡检，任务与基本收件仍保留。取消按任务和当前稳定 owner
持久保存；新任务不恢复旧任务取消，同稳定名字换会话保留取消。恢复使用明确入口。
目标命令、回执和 CEO 回报边界见
[board idle 巡检契约](references/board-idle-patrol.md)。

这是 2026-10-25 用户确认的契约。`collab notify cancel --event board-idle`、
`collab notify subscribe --event board-idle` 与 `collab notify status` 已在当前源码
实现。本轮隔离公开入口/native 与有限 timer 兼容验收已通过；这不等于生产 daemon
启用或生产 peer 消费或恢复推理已验证。使用前先按运行 daemon 的实际构建核实
`collab notify methods` 与 `collab notify status`；其能力或状态不满足时只停受影响
自动巡检，已授权工作仍继续。

巡检只提供兜底唤醒，不是执行前提。登记后立即推进 ready 工作，结果或资源变化后按
统一调度规则自检并补位。peer 保留本人范围的交付责任；遇到受限步骤先按已有授权
解阻，及时向真实 master 提供原错、尝试、拟方案与所需决定，只停受影响动作。
master 检查必要证据，不用 ACK 或自报完成代替验收。

完成后关闭本人有限 goal/task 调度并回收自有资源；未完成时保留恢复证据并如实报告。
基本 direct-message 订阅由 daemon 管理，关闭任务或 goal 不取消它。不为保持繁忙扩展
范围，也不为停止提醒伪造完成。

## CEO 与项目进度

全局 CEO 只由 daemon 的 typed grant 授权；工作角色 `ceo` 不授予全局权限。项目
master 有 CEO 时通过现有 `collab ceo reply` 路由回报目标、已验收结果、剩余工作、
阻塞和下一动作；无 CEO 时明确报告 `CEO_NOT_CONFIGURED`，继续本项目工作。

CEO 按项目和实质业务进展去重。非紧急汇报默认每小时最多一次，安静窗口默认
`Asia/Shanghai` 每日 `[00:00, 08:00)`；窗口内累计，结束后合并唤醒。显式
`--urgent` 是唯一 typed 紧急入口，可穿透限流与安静窗口，正文关键词不升级紧急。

`queued`、`accepted`、`consumed` 与 `reported_to_user` 分开记录。读取
`collab ceo progress` 不标记已向用户汇报；只有 CEO 在宿主实际输出后执行
`collab ceo report-ack --batch-id <id>`，才提交 `reported_to_user` 的调用者声明。
每次主动向用户报告前，先读取当前 `ceo progress`，只按其 `report` 批次组织本次报告。
等待或断线后重新读取，不用旧批次或其他项目视图绕过当前静默与小时限制。
该声明不独立证明用户实际收到或已读。目标接口实现验证见
[board idle 巡检契约](references/board-idle-patrol.md#ceo-进度回报与用户报告边界)。

CEO workspace 的默认偏好与写作/汇报风格指引由 `collab ceo promote` 非破坏性写入
或更新 `<workspace>/AGENTS.md` 受管段落，引用 `~/.codex/USER.md` 而不复制内容或
个人信息；非法目标显式失败，不写备选文件。不新建第二套 CEO Skill。
CEO 按该指引维护用户明确确认的稳定偏好与写作习惯：先读取权威偏好源，去重后做小量
更新。不把单次情绪、项目进度或秘密写成用户偏好。

## 项目稳定名字与工作继承

按 [稳定名字恢复](references/stable-name-recovery.md) 和
[名字迁移入口](operations.md#稳定名字与旧版-display-label-迁移)处理用户明确要求的
身份操作。当前 context 是事实；项目历史名字表只是索引。没有实际名字时不认领历史
身份、不等待分配。

## 资源和生命周期边界

每个 peer 只拥有获授权的任务、文件、worktree 和资源。并发写入前按
[任务与 worktree](references/task-worktree-lifecycle.md)登记；冲突按
[资源等待](references/resource-waits.md)隔离，不覆盖或回收他人的资源。显式取消、
集成、清理沿各自 owner 的入口。

Collab daemon 由用户管理。普通协作、安装、注册和恢复不包含 daemon 维护或任何 tmux
server/pane 生命周期操作。选定传输失败后不跨传输伪造成功。生产工作不挂到未交付的
v2 实验状态。

源码、安装、用户激活和 live 行为分别验收；操作路径见
[唯一 runtime 交付 DAG](operations.md#collab-runtime-delivery-dag单源单汇)。
日常发送消息不加载安装或发布流程。

## 按需操作索引

| 当前动作 | 只读对应内容 |
|---|---|
| 修改角色或工作流程模板 | [角色分工](operations.md#工作角色与用户指定分工)、[流程模板](operations.md#唯一-skill-与当前工作流程) |
| 任务板发布、认领、交付及队列接续 | [任务板与 dashboard](operations.md#public-task-board-and-read-only-dashboard) |
| 通信失败、身份恢复或显式重置 | [Recovery / Failure / Reset](operations.md#recovery-failure-reset-regression)、[迁移边界](references/migration-daemon.md) |
| CEO、跨项目 master 操作 | [CEO](operations.md#全局-ceo-与项目-master)、[跨项目通信](operations.md#cross-project-master-communication) |
| board idle 巡检、取消与 CEO 回报 | [board idle 巡检](references/board-idle-patrol.md) |
| 宿主注册、AppServer 或 thread 路由 | [宿主注册](operations.md#三种宿主的注册方式与锚点dsh--codex-tui--codex-desktop)、[AppServer](operations.md#appserver-runtime-registration) |
| 安装及受影响 runtime 验收 | [构建安装](operations.md#current-version-baseline)、[验证细节](references/verification.md) |
| 通知、订阅、路径问题 | [通知](references/notifications.md)、[状态路径](references/state-paths.md) |

源包更新可能重写本机 Skill；本次正文变更不代表 AppSDK 分发源或运行二进制已经更新。
涉及分发时由对应项目任务同步，不能将文档修改当作 runtime 修复。
