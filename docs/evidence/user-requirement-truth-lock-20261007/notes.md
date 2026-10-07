# 用户需求真相锁设计：阶段记录

2026-10-07 / 范围｜本轮仅完成设计、项目 graph、gap 和 goal 交接；不改产品代码、不启动 goal｜用户本轮指令；`docs/design/user-requirement-truth-lock.md`｜源码基线 `c3c0c8df79e69534fe30c92db61328824473d5c0`，AppSDK main 初始 clean｜核查现状。

2026-10-07 / 核查｜确认目标校验只检查确认字段存在；已有目标快照/Guidance 绑定与冻结历史；未证明用户独占授权能力｜`rust/src/main/registry.rs:696`、`compile.rs:1105`、`guidance.rs:401`、设计 G1–G8｜同一源码基线；静态只读证据｜形成需求变更与消费两条 SESE 设计。

2026-10-07 / 设计｜两条独立来源各自单源单汇；用户授权、版本持久化、消费 gate、保留和 T1–T15 验收已写入设计；授权入口仍 UNVERIFIED，独立设计 review 尚未执行｜设计及两份 graph｜绑定名称为拟定名称，无运行实现｜验证拓扑及文档一致性。

2026-10-07 / 图验证｜两份 graph 的 validate、inspect 均成功；每图 5 节点、4 边、5 波；仅拓扑 PASS｜`graph-validation.txt`｜同一源码基线与本轮设计 graph；canonical installed dagpipe｜保存 goal，并做最终针对性检查。

2026-10-07 / 文档验收｜manifest 唯一 ID 与图路径一致、每图单入口单出口、G1–G8 与 T1–T15 完整、goal 引用存在且状态表述正确；`git diff --check` 通过｜本轮针对性只读检查输出；`docs/goals/user-requirement-truth-lock-goal.md`｜仅文档/设计图，无产品代码变化｜交付用户；后续执行者先做能力确认和独立设计 review。

2026-10-07 / 资源与交接｜本轮未创建 worktree、临时进程或订阅；设计与证据为需保留的交接产物，未提交｜本轮文件状态及用户设计请求｜当前可信 main 文档编辑；不覆盖既有文件｜用户可复制 goal 开始实现；本轮不启动执行。

2026-10-07 / 用户补充与 owner｜用户要求 reviewer 必须核验权威需求并纳入完整提示词；此前仅设计了授权/消费，未把 reviewer 输入与裁决明确写全｜用户本轮原话；Codex buildReviewPrompt 与 AGY buildReviewPrompt 均读取全局 codex-review 标准和提示词｜现有共享文档为唯一 owner；不改 backend 代码｜修订共享输入/裁决和本任务派单合同。

2026-10-07 / reviewer 合同修订｜全局 review-standards 增加权威用户需求核验；review-prompt 列出必读输入与既有输出记录；项目增加 G9、7.1、T16，goal 同步｜全局 codex-review 两份文档；本任务设计与 goal｜无产品/脚本/图拓扑变更｜验证真实 prompt 生成函数加载新合同；行为裁决尚未执行。

2026-10-07 / reviewer 提示词验证｜分别调用 Codex/AGY 脚本现有 buildReviewPrompt 函数，均包含完整共享标准、输入/输出契约、权威需求核验、准确 scope、只读及 controller 边界；`git diff --check` 通过｜`reviewer-prompt-validation.txt`｜本轮共享文档内容；只读函数调用，无 backend/model 启动｜交付修订；实际 reviewer 遵循与 T16 controller 行为仍 UNVERIFIED。

2026-10-07 / 当前执行与图证据更新｜用户已要求GCM编排执行；消费graph 0.2.0现为6节点5边，validate/inspect PASS，覆盖正式SDK模板组装节点。历史0.1.0笔记保留为历史，当前证据以更新graph-validation.txt为准｜候选graph与graph-validation.txt｜产品尚未编码；静态绑定未注册｜独立设计审查发现readiness/旧review循环，先修设计再审。
