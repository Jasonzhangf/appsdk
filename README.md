# AppSDK

通用项目骨架：模板、阶段治理、变更范围、冻结合同和确定性编译产物。

AppSDK 不包含任何业务协议、provider、项目 pipeline 或运行时实现。新项目拥有业务语义；AppSDK 只提供机制。

默认流程：明确目标/范围 → 实现 → 相关验证 → review → 授权交付。
质量、安全和证据真实性门禁按适用范围强制。Collab 保持多 worker 自动注册、
通信与任务/文件归属；仅协作不强制启用合并队列。Guidance、Memory 默认辅助，
不要求每个任务创建计划、写记忆或清理工作树才能交付。
运行时 review 的安装/重启要求由模块 `deployment_operations` 声明，缺省保留
两项旧要求，`[]` 表示均不适用。该声明绑定产物；黑盒、准确候选和产物证据仍强制。
Review 后复用相同输入/产物的有效证据，发生相关变化才重跑；AI 置信度不再是门禁。
下面的完整运行时冻结流程仅在相应交付范围内使用。

## Development Process Control Harness

Harness 将项目声明的 `AGENTS.md`、Skill 和 JSON 流程合同编译为确定性规则上下文；Agent 生成 Plan，AppSDK 校验并持久化 Plan/Revision/Step event，投影当前状态和唯一相邻下一步。它不调用模型、不复制 lifecycle、不自动写 memory，且默认 advisory。

项目命令默认以当前进程 `cwd` 为项目根，不需要项目根环境变量。只有从项目目录
外操作时才传可选路径。明确选择 Guide 的项目若尚未配置，执行幂等
`appsdk init`，再执行只读
bootstrap intake。Agent 读取返回的项目文档和本地 Skill 候选，向用户提交
`GuidanceSetupProposal`；批准后才写项目规则并 compile：

```bash
cd ./existing-project
appsdk init
appsdk guide init --task guidance-setup --mode bootstrap --module app-core
# user approves the Agent proposal; Agent updates project-owned rule sources
appsdk guide compile
appsdk verify
```

在 live tmux Agent 中，`appsdk init` 同时执行官方 `collab init`：幂等创建
Collab 项目状态、启动单 daemon、注册当前 peer，并建立默认有限
`direct-message` 订阅。无需再运行 `collab whoami` 或手工订阅普通消息。
AppSDK 不选择 Collab 路径；Collab 继承同一环境，并以 tmux pane cwd 作为项目根。
非 tmux 操作没有可注册 peer，AppSDK 只初始化治理并明确输出 Collab pending。

```bash
appsdk guide compile
appsdk guide status --task task-id
appsdk guide init --task task-id --mode develop --module app-core
appsdk guide develop --task task-id --module app-core
appsdk guide plan --task task-id --input plan.json
appsdk guide update --task task-id --input result.json
appsdk guide next --task task-id
appsdk guide close --task task-id
```

`guide init` 只读返回需要先读的项目 AGENTS/本地 Skills、仍需询问用户的问题、Skill 调用建议和下一条 guide 命令。项目级 `GuidanceSetupProposal` 与任务级 `PlanProposal` 分离；`guide plan` 才开始写入任务控制态，任务 Plan 不会自动写入项目 Skill。

功能边界见 [Development Process Control Harness](./docs/architecture/development-process-control-harness.md)，详细合同见 [Harness design](./docs/design/appsdk-guidance-framework.md)。

## 第一阶段治理流程

```text
Goal clarification -> clean worktree -> reproduce -> fix candidate -> development whitebox PASS -> build/install/restart -> deployed blackbox PASS -> review admission -> architecture PASS -> effectiveness replay -> tested integration -> local/remote mainline receipt -> compile library
                  -> Active lib -> Protected source/contracts -> lock -> close
```

核心命令：

```bash
appsdk prepare ./existing-workspace
appsdk init ./existing-workspace --project-root new-code
appsdk new ./my-app
appsdk pin-lock --binary /path/to/appsdk
appsdk compile
appsdk rehydrate-frozen --module app-core
appsdk begin-version --module app-core --from active-v1 --to active-v2
appsdk verify
appsdk verify --review-admission --module app-core
appsdk promote --to source_implemented
appsdk promote-module --module app-core --to architecture_stable
appsdk freeze --module app-core
appsdk publish-active --module app-core --version active-v1

```

`appsdk prepare` 先创建初始化需求模板。AI 读取模板并与用户确认 change kind、项目根、旧代码边界、新目录、Protected 路径和禁止修改路径；只有 preparation status 为 `confirmed` 时，`appsdk init` 才允许执行。`appsdk init` 用于已有工作区：通过可选的 `--project-root <relative-path>` 将新 AppSDK 项目放进可配置子目录，允许新旧代码共存。它幂等创建治理目录，补齐缺失的 `.appsdk/` 合同文件，并向新项目根目录的 `.gitignore` 追加一次受 SDK 管理的忽略区块；在 live tmux Agent 中还通过同环境官方 `collab init` 完成 Collab/daemon/peer/default direct-message subscription 的一次性初始化。

治理设计：

- [Playground → Active Promotion Contract](./docs/design/playground-active-promotion.md)
- [AppSDK Governance Architecture](./docs/architecture/appsdk-governance-architecture.md)
- [Lifecycle Record Contract](./docs/design/lifecycle-records.md)
- [Zone Transition Matrix](./docs/architecture/zone-transition-matrix.md)
- [Goal Clarification Contract](./docs/design/goal-clarification-contract.md)
- [AppSDK Project Integration](./docs/design/appsdk-project-integration.md)
- [Development Scenario Contracts](./docs/design/development-scenarios.md)
- [Development Process Control Harness](./docs/architecture/development-process-control-harness.md)
- [Harness Detailed Design](./docs/design/appsdk-guidance-framework.md)
- [Rust Binary Delivery](./docs/design/rust-binary-delivery.md)

目标提示词：收到一个新开发/debug目标时，先澄清并确认目标，再写 `docs/goals/<feature-name>-plan.md`，最后输出短 `/goal`。固定规范见 [`skills/appsdk-project-governance/references/goal-prompt.md`](./skills/appsdk-project-governance/references/goal-prompt.md)。

可复用 Skill：[`skills/appsdk-project-governance/SKILL.md`](./skills/appsdk-project-governance/SKILL.md)。它定义新项目如何引用外部 AppSDK、提交 `.appsdk/` 项目治理合同、忽略 `.appsdk-control/` 本地运行态，并执行 clarification → Playground → review → promotion → freeze。

`goal clarification` 未进入 `confirmed` 前，不允许创建正式 claim、写 Playground、修改 source 或生成 red test。`playground/` 是可变实验源代码；`active/lib/` 是不可变、当前有效的消费面，不是活跃源代码；`protected/` 保存冻结源代码、合同和历史版本；`generated/` 只保存编译物和索引。进入 review 前必须同时有开发白盒 PASS、部署后的公开入口黑盒 PASS，以及绑定候选 commit/tree、artifact、environment 和 entrypoint 的 PreReviewValidationRecord；`appsdk verify --review-admission` 是强制 admission 命令，宿主 CI/pre-commit 需要调用它才能物理阻断 Git commit。进入 Active 还必须有架构 review PASS、主线合并、编译产物和 required gates。锁定必须记录 Git clean、source commit/tag、library hash、public API hash、review PASS、旧 Active 不可变。

多 worker 自动使用 Collab 注册、通信和任务/文件归属。`multi_worker_collaboration` 可独立启用；选择 `multi_worktree_merge_queue` 时才额外要求串行集成及其完整证据。每个 worker 独占 semantic claim、branch 和 clean worktree；worker 不写 main。队列验证精确 integration commit/tree，并在本地与远端 main 都可达后才允许对应 promotion、cleanup 和 claim release。

Playground 不是永久缓存：实验必须在 review/closeout 时归档并清理，默认 `archive_then_remove`；debug 正式合并必须在变更附近留下根因、设计批准 ID 和修改理由注释，并在 PromotionRecord 中留证。

SDK 输出并校验 protected path、module owned path 和 frozen module 合同；真正文件写入由宿主的 worktree/container 或 race-resistant filesystem adapter 执行。没有物理隔离合同，SDK 不得宣称“AI 无法越界写入”。

Protected + Git 负责审计、变更检测和恢复，不保证同仓库 shell agent 完全无法读取 Protected 源码；真正读隔离需要 host worktree/container/mount 策略。第一版只验证治理骨架；RouteCodex 迁移不属于本仓库第一阶段。
