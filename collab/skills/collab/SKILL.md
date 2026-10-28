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
cwd 确定，稳定名字承接工作；sessionID 承载当前执行，tmux pane、thread 和 socket
仅定位通信。不得猜身份、复制 token、编辑控制状态或以 pane 内容证明消息消费。
恢复失败只停受影响协作，保留原任务及资源。

## 日常通信

先按[轻协作与请求边界](../codex-orchestrator/references/orchestration.md#轻协作与请求边界)判断是否需要通信。可自行取得的事实和范围内决定直接处理；board 与结果文件足以推进时不再催报、催读或互答 ACK。只有实际交接、依赖变化、明确答复义务或必需决定才发送消息；完成本人义务后无需例行申请 idle。

| 任务 | 入口与边界 |
|---|---|
| 恢复当前已注册身份和状态 | 在本项目实际环境运行 `collab context`；仅给当前宿主的真实 runtime 信息 |
| 本项目内向明确接收者发送已授权消息 | `collab sendmessage --to '<真实名字或from>' --subject '<主题>' '<正文>'`；已有接收者时直接发送，不先轮询对方是否空闲，不把跨项目地址传给 sendmessage |
| 向另一项目交付资料或消息 | 先读 [操作手册](operations.md#cross-project-master-communication) 的跨项目章节；源项目当前 master 用 `master send`，目标以 `collab master status` 取当前 `master.worker_id`，普通 peer 交本项目 master 处理 |
| 收件与消费 | 非 DSH 用 `collab recv`；DSH gateway 独占 Poll/Ack，直接处理原生完整正文，不运行 recv/ack |
| 只读查消息 | `collab msg <真实消息ID>` 或 `collab inbox`；只读查询不等于消费 |
| 只读查项目通信目录（未注册可用） | `collab directory`；只读返回已知接收方白名单，不写 presence，不暴露凭据或任务细节 |
| 匿名发送通知（未注册可用） | `collab sendmessage --unknown-sender --to '<目标peer>' [--target-session '<会话>'] --subject '<主题>' '<正文>'`；固定来源为 unknown，跳过身份获取，无回信义务 |

### 收件与继续工作

Collab 消息由 daemon 按当前绑定投递通知。非 DSH 宿主收到通知后执行一次
`collab recv`，处理收到的消息，再继续本人可执行的工作。没有通知时，不运行
`recv --timeout`、`inbox` 或后台循环来等消息，也不反复向用户汇报“正在等待”。
等待子 agent 或构建的规则不改变 Collab 收件方式。

委托或请求其他 peer 后，继续推进、验证或接管已授权的独立工作。只有确有外部
依赖且没有其他可执行动作时，记录具体阻塞和下一动作，结束当前回合，待通知或新
信息到达后再继续；不推断对方正在工作。通知未到或传输失败时报告真实状态，不把
消息自动到达当作已验证事实。

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

master 对项目、peer 对受托目标都是最终审核人和责任人；日常推进开发/验收/集成、P0/P1
清除及自有资源和闲置 worktree 释放。board 空或一次任务交回不代表责任结束；有已授权
缺口就继续，真实阻塞只停受影响工作。CEO 直接读 board 监督，不依赖例行报告；项目状态
只面向 CEO，技术文件交回不增加汇报链。确实需要用户专属事实、新授权或硬约束决定时
可以直接询问用户，说明证据和受影响范围，其余工作继续。完整责任与阻塞证明见
[负责人日常推进与最终验收](../codex-orchestrator/references/orchestration.md#负责人日常推进与最终验收)，
独立 review 不能被负责人自验替代。

master 管项目目标，peer 管已接受的子目标，两者均负责拆分、解阻、验收和资源收口。
长程目标经任务板邀请可用且同意接收的 peer；一次性执行或观察沿
[统一角色路由](../codex-orchestrator/SKILL.md#codex-路由)。
无可用 peer 时保留长程目标并拆分 worker，不等待注册。

派单使用 [Worker 合同](../codex-orchestrator/references/worker-contract.md#任务合同模板)，
明确允许/禁止、验收、升级与停止点；peer 另带任务 ID、稳定双方身份、目标层级和
集成分工。范围内投入和预算边界不另设一套。Planner/Reviewer 模型、并发调度、接管
与 Review 准入均沿 [codex-orchestrator](../codex-orchestrator/SKILL.md)；任务板记录
本身不授权范围外 backlog 或生产动作。

父子 agent 的派单、续派、返工、换负责人及结果交回使用
[统一文件模板](../codex-orchestrator/references/worker-contract.md#任务合同模板)。
设计、规划和审查者亲自写清关键决定，消息只传可读路径和必要差异，不复制报告全文。

通过 Collab 编排的独立子任务必须在现有 board 中维护父子关系、当前状态与合同/结果
引用；实际执行的一次性 worker 由注册编排者跟踪，不额外注册。普通查询与通知不建任务。
操作见[文档目录与 board](operations.md#文档目录与-board-父子任务跟踪)。

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
项目存在可推进的未关闭任务或 worker 正在执行时，须确保持续巡检 armed，并复用同一
订阅而不并排新建计时器。有任务时必须持续巡检，任务恢复可执行、收到新任务或等待输入
解除后立即重新开启。没有任务进展且已向 CEO 或需求方上报，或正在等待用户/外部输入且
该状态已上报时，可以停止本人有限巡检；记录原因、涉及任务和恢复条件，保留 direct-message。
停止可用当前 master 的 `collab notify cancel --event master-idle` 显式取消，条件与回执
见下文契约及 [board idle 巡检契约](references/board-idle-patrol.md)。任务集合为空只表示
没有对应 worker 唤醒对象；负责人仍须检查项目目标、缺陷和资源收口，不能据此结束项目职责。
每次提醒检查 worker 的 busy/idle、board 任务和按任务取消状态。当前 master 用
`collab notify status` 的 `project_board_patrol` 只读视图检查 peer 取消；该读取不授予
替 peer 恢复或覆盖取消的权限。peer 持有未关闭
board 任务且正态 idle 时应唤醒本人，由本人继续推理并判断下一步；busy 跳过，不
补发；unknown、offline、cold 或 waiting_user 不当作 idle。支持实时状态事件时及时
触发；不支持时默认每 15 分钟检查（当前 `notify status` 报告 realtime 未接通）。

peer 可取消本人 board 巡检，任务与基本收件仍保留。取消按任务和当前稳定 owner
持久保存；新任务不恢复旧任务取消，同稳定名字换会话保留取消。恢复使用明确入口。
当前 master 也可取消本人的常驻项目巡检：`collab notify cancel --event master-idle`
只在项目已有 `ceo_report_state` 进度且无可执行未关闭项目任务时成功，成功回执返回
原订阅 ID 与 `status=cancelled`；重复取消幂等。显式精确订阅或项目工作变化会 rearm
同一订阅，direct-message 始终独立保留，peer board idle 取消语义不变。`notify status`
的 `project_patrol` 只读投影给出 armed/cancelled/missing。
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

CEO 负责检查与用户即时沟通的通道是否已安装、完成登录、绑定真实用户且可双向通信。
用户指定的 QQBot 通道可用后，紧急事件、工作汇报和需要用户决定的仲裁请求通过该即时
通道发送；无变化不例行发送，普通汇报继续遵守已有频率和静默约束。不可把“Skill 已
安装”“扫码页已打开”或发送入队当作实际送达。保留真实发送回执，用户决定须关联原任务
与具体问题，不能由沉默推断批准。

通道未连接、登录过期或发送失败时如实记录原错和待发送事项，通过当前可用用户会话
说明缺口，不谎称已通知，也不让无关已授权工作停下。仅向用户确认或登录后明确绑定的
目标发送，不猜 QQ 号、收件人或凭据，不把 secret 写进文档。通道 Skill/CLI 的实际入口
核实后再填写，不凭空编造登录与发送命令。

master 和 peer 的项目进度、风险、阻塞与完成状态唯一汇报对象是当前获授权 CEO，由 CEO
面向用户。父子任务的结果文件交回、指定独立审查和必需的执行协调继续按合同处理，不是
新增进度汇报层级。CEO 通道不可用时记录该路径缺口并继续独立工作，不把项目停在“等
用户安排”。

全局 CEO 只由 daemon 的 typed grant 授权；工作角色 `ceo` 不授予全局权限。CEO 在自身
已授权 workspace 使用 `collab ceo progress`（可加 `--project '<项目根>'`）直接巡检各
项目公开 board。读取 `projects[].tasks`、父子关系、状态、验收/合并/清理缺口、下一步
和文档路径，不等待 master 例行汇报。master/peer 及时维护 board 与文档；取消“master
周期汇报→CEO 再汇总”的循环，不发例行进度消息。明确请求仍须回复，需 CEO 决策的权限、
范围、资源冲突和紧急异常仍按授权通道提出。无 CEO 时明确报告 `CEO_NOT_CONFIGURED`，
继续本项目工作。

CEO 复用自身已有巡检调度查看 board；仅处理实质进展、停滞、失败、责任缺口和所需决定，
无变化安静，不向 master 例行催报，不新建每项目汇报循环。当前 `progress.projects` 的
board 读取已存在；`progress.report` 仍由旧 master 进度记录驱动，不等同 board 变更汇总。
未改程序前不得宣称 board 自动触发 CEO 用户报告，也不得伪造 report-ack。

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
