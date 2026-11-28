<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T15:07:02.770774+00:00","entry_kind":"rule","id":"appsdk-collab-auto-dispatch-ticket-system","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["daemon 按接收范围入最短队列","review 按 codex-auto-review、reviewer、peer、Jenny 顺序补位"],"axis":"behavior","axis_id":"collab.board.auto-dispatch","clause_id":"collab.ticket","effects":["idle 提醒直属 manager"],"evidence":["docs/design/collab-ticket-implementation-design-20261107.md"],"forbidden_actions":["入队本身提醒","巡检员与记忆 agent 进入普通派发候选","引入独立运行数或快照缓存"],"owner":"Collab board/daemon owner","rule":"board 是自动派单工单系统：daemon 自动派单、排序、转交、唤醒；执行/plan/review 各有队列；宿主优先级 TUI、DSH、Desktop。","scenario":"有 ready 任务且有容量","scope":"AppSDK Collab board","state_after":"工单在阶段队列中被分配","state_before":"工单待派","trigger":"工单入 daemon 或阶段推进"},"revision":1},"source_refs":[],"tags":["appsdk","authority","board","collab","daemon","dispatch","plan","queue","review","ticket"],"updated_at":"2026-11-13T23:21:45.811710+00:00"} -->

# Collab board 升级为自动派单工单系统

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论）：

1. master 建单、分类、调整优先级与接收范围，master 可关闭工单；peer 可自建自领、claim/update，无删除与关闭权。
2. daemon 自动派单、排序、转交、唤醒与阶段推进；入 daemon 即按接收范围进入最短 slot queue；busy 与依赖未满足不阻止入队，仅开始执行前检查。
3. 执行、plan、review 各自拥有独立队列；对应阶段的单进入后自动分配到最短 queue 的 worker（用户 2026-11-07 补充确认）。
4. 需 plan 的单先交 plan 团队，规划完成后再分发给执行者；需 review 的单在作者满足准入后自动转 review 角色。
5. 审查路由（2026-11-11 用户修正）：普通 review 默认用独立 codex-auto-review；
   其次 reviewer role；再次普通 peer；最后 Jenny。缺 reviewer 时按同一优先级
   补位；review 排除作者、Planner、实现者及内容修改者。
6. 提醒合约：运行中任务变更直接提醒当前执行者；入队本身不提醒；worker idle 提醒直属 manager；manager idle 提醒巡检员；worker 同一 idle 阶段严格超过 10 分钟同时提醒 manager 与巡检员。
7. peer 失联（2026-11-12 用户修正）：状态探测沿 `appsdk-collab-adaptor-lifecycle`。
   daemon 确定身份丢失后的回归等待与超时回收重派，统一沿
   `appsdk-collab-task-offline-reclaim`；原名字和绑定保留恢复入口。等待期间继续独立工作。
8. 巡检员与记忆 agent 从普通派发候选排除；master 默认承担管理职责。
9. 不引入独立运行数、队列长度、个人/团队 queue、快照缓存或推导授权；团队与个人视图读取同一阶段记录。
10. 设计已通过独立审查，待 Jenny 组织实施；对应文档：
    `docs/design/collab-ticket-implementation-design-20261107.md`、
    `docs/dagpipe/collab-ticket-flow-20261107.md`、
    `docs/plans/collab-ticket-system-20261107.md`、
    `docs/dagpipe/collab-ticket-current-20261107.md`。
11. 宿主派单优先级（2026-11-11 用户修正）：Codex TUI（rex 与 tmux 宿主）优先，
    其次 DSH，最后 Codex Desktop。抽象依据：TUI 是可观测的 TUI-based agent，
    一个容器可以承载多个 peer，输入输出容易控制；DSH 与 Codex Desktop 依赖
    bridge，通信更麻烦。Codex Desktop 通常承担 architect 类型工作，派给它
    主要是 review。

该决策是 board 的目标架构；在实现合入前，不把旧简单 board 当作长期权威设计。

Ontology（2026-11-11 翻新）：

- 场景：有 ready 任务且存在可用容量，需要选择派发宿主。
- 触发：准备把任务派给某个 peer/宿主。
- 允许动作：按 TUI（rex/tmux）→ DSH → Desktop 的优先级派单；TUI 的状态鉴别
  沿 `appsdk-collab-adaptor-lifecycle` 当前已批准顺序；Desktop 主要承接 architect/review 类工作。
- 影响状态：派单进入可观测宿主；丢失判定、身份回归等待及任务回收沿
  `appsdk-collab-task-offline-reclaim`，保留未被替换身份的恢复关系。
<!-- project-memory:end -->
