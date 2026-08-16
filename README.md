# AppSDK

通用项目骨架：模板、阶段治理、变更范围、冻结合同和确定性编译产物。

AppSDK 不包含任何业务协议、provider、项目 pipeline 或运行时实现。新项目拥有业务语义；AppSDK 只提供机制。

## 第一阶段治理流程

```text
Goal clarification -> clean worktree -> reproduce -> fix candidate -> architecture PASS -> effectiveness replay -> verified mainline merge -> compile library
                  -> Active lib -> Protected source/contracts -> lock -> close
```

核心命令：

```bash
appsdk prepare ./existing-workspace
appsdk init ./existing-workspace --project-root new-code
appsdk new ./my-app
appsdk pin-lock ./my-app --binary /path/to/appsdk
appsdk compile ./my-app
appsdk begin-version ./my-app --module app-core --from active-v1 --to active-v2
appsdk verify ./my-app
appsdk promote ./my-app --to source_implemented
appsdk promote-module ./my-app --module app-core --to architecture_stable
appsdk freeze ./my-app --module app-core
appsdk publish-active ./my-app --module app-core --version active-v1

```

`appsdk prepare` 先创建初始化需求模板。AI 读取模板并与用户确认 change kind、项目根、旧代码边界、新目录、Protected 路径和禁止修改路径；只有 preparation status 为 `confirmed` 时，`appsdk init` 才允许执行。`appsdk init` 用于已有工作区：通过可选的 `--project-root <relative-path>` 将新 AppSDK 项目放进可配置子目录，允许新旧代码共存。它幂等创建治理目录，补齐缺失的 `.appsdk/` 合同文件，并向新项目根目录的 `.gitignore` 追加一次受 SDK 管理的忽略区块。

治理设计：

- [Playground → Active Promotion Contract](./docs/design/playground-active-promotion.md)
- [AppSDK Governance Architecture](./docs/architecture/appsdk-governance-architecture.md)
- [Lifecycle Record Contract](./docs/design/lifecycle-records.md)
- [Zone Transition Matrix](./docs/architecture/zone-transition-matrix.md)
- [Goal Clarification Contract](./docs/design/goal-clarification-contract.md)
- [AppSDK Project Integration](./docs/design/appsdk-project-integration.md)
- [Rust Binary Delivery](./docs/design/rust-binary-delivery.md)

目标提示词：收到一个新开发/debug目标时，先澄清并确认目标，再写 `docs/goals/<feature-name>-plan.md`，最后输出短 `/goal`。固定规范见 [`skills/appsdk-project-governance/references/goal-prompt.md`](./skills/appsdk-project-governance/references/goal-prompt.md)。

可复用 Skill：[`skills/appsdk-project-governance/SKILL.md`](./skills/appsdk-project-governance/SKILL.md)。它定义新项目如何引用外部 AppSDK、提交 `.appsdk/` 项目治理合同、忽略 `.appsdk-control/` 本地运行态，并执行 clarification → Playground → review → promotion → freeze。

`goal clarification` 未进入 `confirmed` 前，不允许创建正式 claim、写 Playground、修改 source 或生成 red test。`playground/` 是可变实验源代码；`active/lib/` 是不可变、当前有效的消费面，不是活跃源代码；`protected/` 保存冻结源代码、合同和历史版本；`generated/` 只保存编译物和索引。进入 Active 必须有 Playground 证据、架构 review PASS、主线合并、编译产物和 required gates。锁定必须记录 Git clean、source commit/tag、library hash、public API hash、review PASS、旧 Active 不可变。

Playground 不是永久缓存：实验必须在 review/closeout 时归档并清理，默认 `archive_then_remove`；debug 正式合并必须在变更附近留下根因、设计批准 ID 和修改理由注释，并在 PromotionRecord 中留证。

SDK 输出并校验 protected path、module owned path 和 frozen module 合同；真正文件写入由宿主的 worktree/container 或 race-resistant filesystem adapter 执行。没有物理隔离合同，SDK 不得宣称“AI 无法越界写入”。

Protected + Git 负责审计、变更检测和恢复，不保证同仓库 shell agent 完全无法读取 Protected 源码；真正读隔离需要 host worktree/container/mount 策略。第一版只验证治理骨架；RouteCodex 迁移不属于本仓库第一阶段。
