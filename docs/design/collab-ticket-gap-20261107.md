# Collab 工单系统主线与目标差距

本文件对照 `../dagpipe/collab-ticket-current-20261107.md` 与目标 `../dagpipe/collab-ticket-flow-20261107.md`。具体机制见 `collab-ticket-implementation-design-20261107.md`，执行顺序见 `../plans/collab-ticket-system-20261107.md`。

## 基线分层

| 层次 | 本轮可确认内容 | 使用边界 |
| --- | --- | --- |
| 当前工作树 | 保留分支 `codex/preserve-appsdk-local-20261021`，存在其他 owner 修改；GCM 报告记录旧树入口和差异 | 不作为实现基线，不带入他人 dirty 内容 |
| `origin/main` | Planner 亲读当前 board、角色、绑定、审查、提交、活动及 context owner | 本方案的源码实施基线；来源写为 `origin/main:path:line` |
| installed | 未查询 | 版本及包含能力未知 |
| live | 未连接 daemon，未执行公开行为 | 所有目标运行行为未知，不能用源码或设计校验代替 |

## 逐项 gap 与验收

表中 owner 为产品唯一代码 owner。Jenny 安排具体 peer/worker；设计 owner 负责本包目标对齐和设计 review。

| 目标节点或边 | 当前源码与可复用部分 | 具体 gap | 唯一 owner / 依赖 | 可观察验收 |
| --- | --- | --- | --- | --- |
| 新单入 daemon 即自动分配 | `origin/main:collab/src/server/board_handlers.rs:315` 已允许 peer 建单并保存 pending | 创建未在同一提交中按接收范围进入最短 queue | board admission；依赖调度状态模型 | 一次 publish 返回稳定工单及 queue 位置；busy 接收者仍可入队；无候选返回持久待分配原因 |
| slot 最短 queue | `origin/main:collab/src/server/binding_slot.rs:186` 提供稳定绑定；`work_assignment.rs:318` 仅比较有无未结责任 | 无阶段队列、水位原子选择及同水位轮转 | `work_assignment.rs` 调度 owner；依赖 State 事务 | 队列水位包含全部未开始项及依赖等待项，不含 running；竞争入队不会都使用旧水位；同水位按轮转默认值选取 |
| 优先级与依赖只限制执行 | `origin/main:collab/src/server/state.rs:38`、`board_execution.rs:14` 可复用资源等待判断 | 无通用阶段依赖列表与可执行优先级选择 | 调度 owner；依赖阶段记录 | 高优先级依赖未满足仍先入队；idle 取下一可执行项；同优先级按入队序 FIFO |
| 自建自领 | peer publish 和 claim 已存在，见 `board_handlers.rs:324`、`board_execution.rs:31` | 需保留本人 slot 限定且完成自动入队，不能直接标运行 | board admission / 调度 owner | peer 一次自建自领落本人合格 slot；busy 不拒绝；本人 stale 后解除本次自领限定并回系统 |
| master 与本人更新，只有 master 关闭 | `origin/main:collab/src/server/board_handlers.rs:369` 描述仅 owner；`mod_parts/part_09.rs:586` 允许 owner 关闭 | master 更新能力需统一；普通/force/旧入口关闭权限不符合目标 | board 与 lifecycle owner | master 更新下属单成功；本人更新自己单成功；他人更新、peer close、peer force close、用 update 写终态均拒绝且无副作用 |
| plan-before-execution | WorkPolicy 已存在，`origin/main:collab/src/board.rs:73` | 无规划阶段、有效计划结果和执行前置；适用计划审查缺阶段 | 阶段推进 owner；依赖 admission/queue | 需规划单只能先到 plan slot；无有效计划不能执行；规划完成及适用计划审查通过后自动排执行 |
| 三档 review 路由 | `origin/main:collab/src/server/required_review.rs:1` 支持指定 reviewer；`work_roles.rs:45` 支持名字专长 | 未自动按 Reviewer → architect/expert → 独立 peer 路由；无范围内临时审查授权 | required_review owner；依赖阶段与角色候选 | 三条路由均可完成；第一组合格但 busy 时仍留第一组排队；三组均缺则等待；交叉 reviewer 不能审其他单或关闭 |
| 审查独立性与实际角色 | `origin/main:collab/src/server/mod_parts/part_09.rs:41`、`:55`；被审版本在 `required_review.rs:37` | 当前 owner/master 可自审；角色或 session 变化不能抹掉实际参与 | required_review owner；依赖稳定参与者记录 | 作者换 session/专长仍被排除；结果保存实际 actor、名字、角色组、顺位和审查主体版本；内容修改使旧 PASS 失效 |
| 关闭前自动送审 | `origin/main:collab/src/server/required_review.rs:89` 只提供缺失 gate | 无持久关闭意图和缺审时自动路由；必需审查可能仅返回错误 | lifecycle owner；依赖 review 路由 | master close 保存意图并自动送审；FAIL 不关闭；当前主体 PASS 且交付条件满足后自动完成原关闭意图 |
| 存量接入唯一、并发去重 | 任务已有原 ID；`origin/main:collab/src/server/mod_parts/part_02/commit.rs:82` 有 command receipt | 未将原单身份、请求意图、接入标志及唯一阶段入队一起提交；同 command/operation 不足以识别不同 payload | admission + commit owner；先于迁移 | 同请求重放、不同请求并发接入同单均只留一单一阶段队列项；同 request ID 不同意图、旧版本和身份冲突明确拒绝 |
| 持久化跨字段一致 | `origin/main:collab/src/server/current_state.rs:53` 原子替换已有；`:84` 区分目录耐久性未知 | 需把队列、claim、结果、下游、slot 释放和提醒意图纳入同一 candidate；不能多个小提交拼接 | State/commit owner；所有写入依赖 | 提交前失败不产生部分新状态；替换后耐久性未知保留同一内存状态和请求回执，重放不重复派发；恢复后不出现双占用或丢责任 |
| claim 与真实运行分离 | `origin/main:collab/src/server/board_execution.rs:57` claim 直接 working；邀请接受同样见 `board_invites.rs:132` | 队列取出、claim 预留、合同投递与实际开始混在一起 | execution owner；依赖 queue | idle 自动 claim 仅占一个 slot；adapter accepted 不算 running；当前 attempt 的 Started 回执才标运行；重复回执不重复推进 |
| 队列自动接续，无入队提醒 | `work_assignment.rs:369` 走邀请消息 | 自动排队若复用邀请将产生多余提醒及人工 accept | 调度与通知 owner；依赖 claim | busy 入队没有给 worker/manager 的入队消息；idle 自动启动；已 idle 时新入队也触发接续；只在真实执行派发时交付合同 |
| 运行变更直接提醒 | `origin/main:collab/src/server/board_handlers.rs:379` 已使审查失效 | 缺少同批保存变更和当前执行绑定的直达提醒 | board mutation + notification owner | 正在运行的任务内容/范围/验收/优先级等执行合同变更，立即生成当前版本提醒；同版本重放不重复；非运行入队不提醒 |
| worker idle → manager，manager idle → 巡检员 | `origin/main:collab/src/server/keepalive.rs:321`、`:699`；`notification_state.rs:92` 为 master 累计模型 | 接收关系层级不足；manager busy gate 会延迟新增必需提醒 | activity/notification owner；依赖可信活动与接收关系 | worker 真实 idle 当次生成 manager 消息；manager 真实 idle 当次生成巡检员消息；peer 编排子 worker 时用实际直属关系；缺目标明确展示 |
| 严格超过 10 分钟与恢复去重 | `origin/main:collab/src/server/name_state.rs:75` 已有可信 idle 起点 | 无本目标的双对象阈值提醒、持久 episode 去重及启动取消 | idle episode owner；依赖前项 | 恰好 600000ms 不发；600001ms 且持续可信 idle 时发双方；busy、unknown、offline、waiting_user、绑定变化终止连续区间；重启不重复 |
| master 鉴别 stale | `origin/main:collab/src/server/name_stewardship.rs:49` 检测不可用后通知重新 assign | 不能把 unavailable 自动当 stale；缺本次鉴别请求、真实发送与回复时间关联 | stewardship owner；依赖消息回执 | TUI snapshot 后未知可由 master 判 stale；DSH/Desktop 实际发出后满 10 分钟仍无有效回复才满足超时证据；发送失败不启动计时 |
| stale 原子撤销与重新入队 | `origin/main:collab/src/server/name_state.rs:29` 有 assignment revision；绑定 generation 见 `binding_slot.rs:190` | 无 attempt 代际撤销、全部未结单回收及自动重分配；绑定代际不能代替任务承接代际 | stewardship + execution owner；依赖事务/queue | 包含自建单的全部未结责任回收；旧 claim、Started、结果和队列提交失效；已关闭历史保留；新归属及等待原因明确 |
| 旧执行共享资源隔离 | `origin/main:collab/src/server/board_execution.rs:14` 有资源 gate | 状态撤销不能停止任意 shell 或外部副作用；缺 stale 后资源 hold 与释放证据 | 现有 worktree/resource owner；依赖鉴别和代际 | 新工单立即重新分配入队；冲突共享资源未隔离时不开始；独立阶段继续；旧工作树和未交付内容保留，未知外部结果先核对 |
| context 最新一致快照 | `origin/main:collab/src/server/mod_parts/part_09.rs:1503`、`:1626` 已有 scope 与任务视图；`:1355` 先读 control 后锁 State | 缺 slot queue、阶段/claim/running 分层和统一身份/业务读取边界 | context owner；依赖稳定状态模型 | 一次 context 看下属运行、队列、优先级、依赖及资源等待；同一快照不拼出旧名字与新承接；读取失败不返回空成功 |
| 旧入口及未结责任迁移 | board、task、scheduler 和邀请 owner 均存在 | 若只新增 ticket 分支，会保留绕过 plan/review/queue/关闭权的第二条路径 | 入口适配 owner；依赖全部核心能力 | 旧公开入口复用同一 admission 和 mutation gate；存量运行不重复启动；pending invitation 不变成 running；未结单及消息责任完整保留 |

## 原设计 P1 的实施落点

审查实际角色落到 `required_review.rs` 的路由记录、阶段 claim 和结果授权，不只修改 Mermaid 标签。存量原子去重落到 admission 和 `commit_command_locked` 的同一 State 候选提交。stale 安全落到任务 attempt fence、现有资源 gate 及资源 owner 的隔离证据；不能宣称 task fence 能阻止任意外部进程写入。

本轮已把旧观察中的三项过时判断剔除：主线已支持 peer 建单、直接 claim、管理范围 context。身份 slot、专长和指定 review 也已有 owner。缺口以本表为准，不重复实现这些基础能力。
