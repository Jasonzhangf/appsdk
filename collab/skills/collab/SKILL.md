---
name: collab
description: "未注册调用者可只读查询项目通信目录与匿名发送自通知；已注册 peer 执行协作协议与任务分工；master 权限需另行授权。未注册无需遵守 peer 协作规程。"
---

# Collab

本入口保留日常协作规则。命令细节按下表读取 [操作手册](operations.md) 的对应章节，
不整篇加载；手册与本入口属于同一个 Skill。

## 适用身份

未注册调用者可直接使用 `collab directory` 查询通信目录与 `collab sendmessage --unknown-sender` 匿名发送通知，无需初始化身份或遵守 peer 协作规程。
协作协议与任务分工仅约束在当前项目成功注册的 peer；master 还须有当前项目的明确授权。
加载 Skill、多个 worker 同时运行、看到历史名字或收到普通派单，都不产生注册或
master 权限。只有用户要求注册时才初始化；未注册或初始化失败时保留原错，独立完成
可做任务，不寻找或等待 master。

已注册身份、角色、任务、消息和订阅以当前 `collab context` 为真源。项目由本次真实
cwd 确定，名字是 TUI、Codex Desktop 和 DSH 唯一的工作归属、身份、投递与任务锚点；
sessionID 只承载当前执行。第一次绑定分配固定名字，后续绑定与恢复可显式继承名字，
新注册为新会话新增名字，旧名字在旧持有者离线或获得用户授权时可顶替。TUI 的固定
tmux session 名字为 `<cwd末级folder>_<manifest固定位置序号>_<Collab名字>`，如
`appsdk_1_Jenny`；只有固定位置 1 可升 Jenny。布局读取共享 manifest，正常改名由
Collab 负责。身份仅从当前 `~/.collab/identity-map.json` 恢复；全部 journal 及旧操作
恢复链已删除。tmux `$N`、pane、thread 和 socket 仅定位通信。不得猜身份、
复制 token、编辑控制状态或以 pane 内容证明消息消费。
恢复失败只停受影响协作，保留原任务及资源。

## 关闭门禁：未验证不得关闭

board issue 和 Collab task 只有在适用证据齐备后才能 `close`。关闭前逐项核对：

1. `completion_missing` 必须为空。仍含 `review`、`integration`、`manual_termination`、
   `cleanup` 或其他未完成项时不得关闭。
2. 代码任务必须有真实入口或公开接口的黑盒证据；单测、build、候选分支、review PASS
   或 merge 分别只证明对应范围，不能替代运行验收。
3. 代码候选只有在对应 owner 已 merge、push 并核对远端后，才能走 integration/close；
   `task close` 在 pending merge 下失败是正确行为，不是可绕过错误。
4. Collab runtime/daemon/安装包变更按
   [Collab runtime delivery DAG](operations.md#collab-runtime-delivery-dag单源单汇)，
   安装、live 激活和受影响真实行为未完成时报告 `INCOMPLETE`，不能标 closed。
5. 只读诊断或无候选任务仍需真实回执、消费证据和 owner cleanup，之后按正常关闭。
6. `--force` 只用于已授权取消或已验证 lifecycle cleanup，且必须写清原因和 cleanup
   receipt；不能为停止提醒、清理未完成工作或掩盖 blocker 而 force-close。
7. 已错误关闭且仍缺证据的任务先 reopen，再补验收；不得删除历史、重写正文或伪造
   回执。已关闭任务保留历史，不把补验写成重新交付。

## 恢复身份后不等待派遣

`collab context` 恢复身份不是等待派遣的信号。peer 恢复后立即继续并推进本人已授权的
工作：已接受任务、本人 goal 和巡检；不因恢复而向 master 报告进度后停下等待派单。
只有确实没有可推进的已授权工作、任务已完成、遇到真实阻塞或需要裁决 scope 冲突时，
才向当前 master 报告；报告和待裁决期间仍继续一切可独立推进的工作，不空闲轮询等待。

## 日常通信

| 任务 | 入口与边界 |
|---|---|
| 恢复当前已注册身份和状态 | 在本项目实际环境运行 `collab context`；仅给当前宿主的真实 runtime 信息 |
| 查可投递地址 | `collab list`；全局目录，不依赖当前目录，输出 `project.name`、通道和真实可达状态 |
| 发送已授权消息（本项目或跨项目同一个命令） | `collab send --to '<project.name>' --subject '<主题>' --content '<正文>'`；地址必须来自 `collab list`，显式地址失败不回退默认目标 |
| 跨项目通信 | 任意两个已注册对象可直接收发，不按 master/peer/CEO 级别限制，也不要求 master 转发。保留真实 sender、精确注册地址与回执。`master send` 只是兼容入口 |
| 收件与消费 | 非 DSH 用 `collab recv`；DSH gateway 独占 Poll/Ack，直接处理原生完整正文，不运行 recv/ack |
| 只读查消息 | `collab msg <真实消息ID>` 或 `collab inbox`；只读查询不等于消费 |
| 只读查项目通信目录（未注册可用） | `collab directory`；只读返回已知接收方白名单，不写 presence，不暴露凭据或任务细节 |
| 匿名发送通知（未注册可用） | `collab send --unknown-sender --to '<目标peer>' [--target-session '<会话>'] --subject '<主题>' '<正文>'`；固定来源为 unknown，跳过身份获取，无回信义务 |

### 未注册脚本自通知

未注册的外部脚本或后台工作进程无需先注册 peer 即可向项目内 peer 发送通知，不适用 peer 协作规程。目录查询和脚本发送都在接收方所属项目根目录运行。

peer 可先运行 `collab directory` 查询自己的 `worker_id` 与 `session_id`，写入后台脚本完成自通知：

- 固定目标 session：指定 `--target-session '<session_id>'`。daemon 在绑定及异步投递时均校验目标 session，peer 换会话后旧脚本明确失败，不误投新会话。
- 跟随 peer 当前会话：省略 `--target-session`，投递时自动跟随 peer 最新有效会话。

```sh
# 例：固定 session 脚本
collab sendmessage --unknown-sender --to 'my-peer-id' \
  --target-session 'session-123' --subject 'build-finished' '构建完成'
```

收到明确请求后处理并按实际回传通道回复，再接续本人未完任务。发送、入队、消费、
回复和完成分别判断；不因超时盲重发，不发无请求的常规心跳、ACK 或进度消息。
一次性 `codex exec` worker 使用派单约定的 stdout/结果文件，不为回报注册 Collab。

宿主 `list_agents` 只枚举该宿主的子 agent，不是 Collab 项目成员目录。列表里没有
某个名字（例如 Jenny）不能证明目标 master 不存在或没有通信通道；当前会话未注册、
缺 master 授权和目标不可达是不同失败，分别以真实 Collab 回执判断。确实要交付资料
给另一项目时，按上表“向另一项目交付资料或消息”走跨项目章节，不因宿主列表无匹配
就默认要求用户人工转发，也不自行注册、猜身份或提升 master。

## Task routing and role responsibilities

master 是项目唯一负责人，负责持续推进、验收、集成和汇报项目进度；peer 缺位或失联不改变 master 的进度责任。peer 管已接受的子目标，两者均负责拆分、解阻、验收和资源收口。
manager 核实下级对进度、完成、阻塞和验收的声明，检查相关原始回执或产物，判断证据
是否支持结论。worker 报告的权限、环境、失败或无法执行也是待核查的声明；manager
检查原错，区分沙箱限制与系统拒绝，并在已有授权内解阻。不把局部限制扩大为全局结论。
master 先按下节当前提示词中的优先级，通过任务板邀请合适、可用且获授权的 peer。
无可承接 peer 时，master 保留目标并按
[统一角色路由](../codex-orchestrator/SKILL.md#codex-路由)拆分独立 worker，不等待注册。

### 派单顺序与一 peer 一任务

manager（master 与各层 peer）每轮派单先查看 board 的当前项目成员与容量，列出全部
idle peer，包括没有当前任务的成员，并显式显示各自通道来源（Codex TUI、DSH、Codex
Desktop）和实际可承接容量。unknown 不算 idle，受保护角色仍显示但不计入业务派单容量。
先按任务权限、能力、依赖和实际容量筛选，再按当前用户或任务提示词中的优先级选择。
派单优先级可直接在提示词自定义，不新增固定宿主排序算法或专用配置。提示词未指定时，
默认 DSH 与 Codex TUI 同层优先，Codex Desktop 最后。有合适且可用的现有 peer 时先派单，
不得跳过 idle 列表直接新开 Desktop peer；没有可承接 peer 时沿已有独立 worker 路由推进。

同一 peer 一次只持有一个未完成实现或主责任务。manager 不把多个并发实现任务叠给同一
peer，也不靠复核为该 peer 加任务；该 peer 交付并关闭当前任务后才接受下一个。候选
peer 与占用状态由 board 的真实任务视图计算。

reviewer、planner、裁判等是职责，不要求存在同名 agent 或固定角色标签。
角色缺位时按[负责人承接规则](../codex-orchestrator/SKILL.md#角色缺位时由负责人继续推进)
改派，并更新原 board 与合同。独立 reviewer 后备、极端条件下的 master 临时代审、
实际审查者与独立性缺口记录均由该规则和现有 Review 合同统一维护。

普通 peer 是受托子目标的编排者，wire role `worker` 不改变该身份。只有当前有效
工作角色明确为 `worker`/`executor`，或任务合同明确指定直接执行时，peer 才在合同
边界内直接执行，不得仅因 peer 身份强制先派子 worker。`patrol` 承担独立常驻检查，
不接收 manager/master 的临时开发派单；`memory` 提炼事实和经验并修正失效结论；
`architect` 产出架构计划及代码准入文件。默认职责正文由源码
[workflow-templates.json](references/workflow-templates.json) 编译，`collab context`
只按当前身份、角色、精确 override 和任务边界投影；本入口不复制默认正文。

派单使用 [Worker 合同](../codex-orchestrator/references/worker-contract.md#任务合同模板)，
明确允许/禁止、验收、升级与停止点；peer 另带任务 ID、稳定双方身份、目标层级和
集成分工。范围内投入和预算边界不另设一套。Planner/Reviewer 模型、并发调度、接管
与 Review 准入均沿 [codex-orchestrator](../codex-orchestrator/SKILL.md)；任务板记录
本身不授权范围外 backlog 或生产动作。

设计、规划与审查角色按
[文件交接合同](operations.md#设计规划与审查角色的文件交接合同)产出并交接落盘文件，
不直接执行实现。

### 跨项目投递与协作边界

任意两个已注册对象可通过统一发送入口直接通信。项目和角色级别不限制普通消息，
不要求同级，也不要求 master 代转。使用自己的真实凭据和目标精确地址。
接收、消费及回信按真实回执记录，不因来源项目或角色拒收。

消息不授予任务分派、项目管理或 CEO 管理权限。执行具体工作仍核对该动作已有的
授权、范围和资源 owner。缺少执行授权时保留消息并向原请求者说明具体缺口；
不伪造已执行，也不借普通发送权限修改他人的 board、汇总或授权。

## Register the long-horizon goal before dispatch

已注册 master/peer 接受长程目标时，各自先恢复或登记本人 goal，再拆解派发。已有目标
直接续接，不重复创建；任务板、笔记和口头承诺不替代 goal 回执。使用当前已支持的
`appsdk goal` 入口，详见
[goal 登记与关闭](operations.md#register-the-long-horizon-goal-before-dispatch)。
master 不代持或代登记 peer goal，旧 runtime 拒绝 peer 时记录具体缺口，不提升身份
绕过。
正式巡检 goal 默认一次登记 100 次，入口保持
`appsdk goal subscribe --goal <已有文档> --interval <既有周期> --repeat 100`；
用户明确指定其他次数时，使用指定次数。
批次仍有效且有剩余次数时复用同一订阅；context、恢复、单次交付或新增轮次都不重复登记、
不重置计数、不并排新建。批次耗尽后，只有原职责仍有效并获授权继续时，由原 owner 整批
续订；显式指定其他次数或普通非巡检 one-shot/deadline 保持原语义。

每个角色维护自己的目标文档。文档先写长期目标、范围、硬约束、交付和验收，再写当前
增量及实现细节。更新细节前回看长期目标；重大细节变化同时修改长期目标。每个巡检任务
和长程任务须有对应 Markdown 日志：接受或恢复时创建或确认，路径关联到目标文档、任务
合同或 board；同一巡检任务各轮续写同一文件，开头长期目标，后续记录本轮目的、来源、
结果、相对前次进展、缺口、阻塞和下一步。文档、board、笔记和角色投影不替代正式 goal
登记回执。

每轮编排循环、board 对齐、优先级与 bottleneck 拆分细节见
[Collab 编排与长程目标](references/orchestration.md)。

## Registered patrol duties

项目 Jenny/master 默认承担本项目 worker 巡检。初始化或恢复本人 longhorizon 后，
须确保本人常驻项目巡检已 armed；它是一条 `master-idle` 订阅，subject
`project-patrol`、目标 interval `1500000` 毫秒（20-30 分钟窗口，默认 25 分钟），
ensure/renew 复用同一订阅而不并排新建。该订阅默认登记 100 次；批次有剩余时
继续复用，不逐轮续订或在 context/恢复后重置计数。批次耗尽后由原 owner 在职责仍有效
且获授权继续时整批续订。`900000` 只兼容已存在的旧订阅；
`collab notify status` 的 `project_patrol.cadence_runtime_gap` 返回
`PATROL_CADENCE_RUNTIME_GAP` 时，必须如实报告运行节奏未达到目标，不能声称已对齐。
有限 goal/task 定时器按生命周期关闭，但单个 goal 结束不关闭这条常驻巡检义务。
每次提醒必须检查四项：未解决的 P0/P1；未完成任务或 ready 工作；自有 worktree 与
资源释放；peer/worker 的 busy/idle 及 idle 原因是否合理。同时检查 board 任务和按
任务取消状态。当前 master 用
`collab notify status` 的 `project_board_patrol` 只读视图检查 peer 取消；该读取不授予
替 peer 恢复或覆盖取消的权限。peer 持有未关闭
board 任务且正态 idle 时应唤醒本人，由本人继续推理并判断下一步；busy 跳过，不
补发；unknown、offline、cold 或 waiting_user 不当作 idle。支持实时状态事件时及时
触发；不支持时由巡检 tick 检查。tick 只发现 due，首次推送要求可信连续 idle 严格
超过 30 分钟，同一 idle 阶段重复推送至少间隔 30 分钟；29m59s 和恰好 30m00s 不推，
不补发漏掉周期。

peer 可取消本人 board 巡检，任务与基本收件仍保留。取消按任务和当前稳定 owner
持久保存；新任务不恢复旧任务取消，同稳定名字换会话保留取消。恢复使用明确入口。
当前 master 也可取消本人的常驻项目巡检：`collab notify cancel --event master-idle`
只在项目已有 `ceo_report_state` 进度且无可执行未关闭项目任务时成功，成功回执返回
原订阅 ID 与 `status=cancelled`；重复取消幂等。显式精确订阅或项目工作变化会 rearm
同一订阅，direct-message 始终独立保留，peer board idle 取消语义不变。`notify status`
的 `project_patrol` 只读投影给出 armed/cancelled/missing、`interval_ms`、
`target_interval_ms` 和 `cadence_runtime_gap`。
目标命令、回执和 CEO 回报边界见
[board idle 巡检契约](references/board-idle-patrol.md)。

这是 2026-10-25 用户确认的契约。`collab notify cancel --event board-idle`、
`collab notify subscribe --event board-idle` 与 `collab notify status` 已在当前
源码实现。`collab notify cancel --event master-idle` 也已在当前源码实现，并定义了
对应的公开黑盒用例；这不等于生产 daemon 启用或生产 peer 消费或恢复推理已验证。
使用前先按运行 daemon 的实际构建核实 `collab notify methods` 与
`collab notify status`；其能力或状态不满足时只停受影响自动巡检，已授权工作仍继续。

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
`collab master status` 与 `collab ceo progress` 的 `workspace` 必须投影当前 master
绑定注册的 canonical project root；不得另造可写状态根，也不得用 cwd 或 session
载体替代工作空间事实。

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
身份操作。当前 identity map 是唯一身份源，context 投影当前结果。没有映射时按本次
真实事实注册，不认领历史身份、不等待分配。tm-iterm 不写 Collab 身份表。

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
| 通信失败、身份恢复或显式重置 | [Recovery / Failure / Reset](operations.md#recovery-failure-reset-regression)、[当前身份与用户维护边界](references/migration-daemon.md) |
| CEO、跨项目 master 操作 | [CEO](operations.md#全局-ceo-与项目-master)、[跨项目通信](operations.md#cross-project-master-communication) |
| board idle 巡检、取消与 CEO 回报 | [board idle 巡检](references/board-idle-patrol.md) |
| 宿主注册、AppServer 或 thread 路由 | [宿主注册](operations.md#三种宿主的注册方式与锚点dsh--codex-tui--codex-desktop)、[AppServer](operations.md#appserver-runtime-registration) |
| 安装及受影响 runtime 验收 | [构建安装](operations.md#current-version-baseline)、[验证细节](references/verification.md) |
| 通知、订阅、路径问题 | [通知](references/notifications.md)、[状态路径](references/state-paths.md) |

源包更新可能重写本机 Skill；本次正文变更不代表 AppSDK 分发源或运行二进制已经更新。
涉及分发时由对应项目任务同步，不能将文档修改当作 runtime 修复。
