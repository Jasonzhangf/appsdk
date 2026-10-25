# DAGpipe 模块职责与导入背景

本页保留一次性源码导入的归属决定，不作为每轮开发、安装或发布的操作清单。
当前 API 与命令见 [DAGpipe README](../../dagpipe/README.md)，选测与交付入口见
[appsdk-dev](../../.agents/skills/appsdk-dev/SKILL.md)。

## 目标与边界

将 DAGpipe `main` 的已跟踪源码（基线 `8cb59fb5262b5e1e440638faf3b04e06a2762cfb`）放入 AppSDK 的 `dagpipe/`，与 `collab/` 并列维护。`dagpipe/` 保持独立 Cargo package、`pipeline_runtime` 库和 `dagpipe` CLI。AppSDK 原有 `appsdk dagpipe` 命令继续负责 AppSDK 自己的 fix/notification Operator 和合同；它只依赖本仓库的 `dagpipe/` 库，不再从 Git 拉取同名库。通用图解析、编译、运行和静态 CLI 的源码 owner 唯一为 `dagpipe/`。

集成是一次性的源码归属迁移：从上述精确提交导入已跟踪文件，之后只在 AppSDK 主链开发和发布，不设置双向同步或独立仓库回写。DAGpipe 源仓库中未跟踪的 `.appsdk-control/` 和设计草稿不进入模块。现有项目图、AppSDK 专属 Operator、`appsdk dagpipe` 命令不搬入通用库。

## 当前模块边界

一次性导入流程不再是业务执行图。当前图按实际请求复用 `docs/dagpipe/` 中的对应对象流；
静态图校验只证明拓扑及声明绑定，真实 consumer 才能证明执行行为。

| 节点 | 唯一 owner | 输入 / 输出与证据 |
| --- | --- | --- |
| 迁入通用引擎 | `dagpipe/` | `pipeline_runtime` 公开 API 与独立 `dagpipe` CLI；原有测试及示例通过 |
| AppSDK 调用 | `rust/` | 本地 path 依赖；现有 `appsdk dagpipe validate`、fix 与 notification 入口可执行 |
| 安装初始化 | `dagpipe/` 安装入口 | 从本仓库构建并安装同源 CLI/SDK/Skill；第二次执行保持同一归属和结果，不创建第二份真源 |
| 交付与清理 | 当前任务 owner | 按宿主交付合同保留实际行为、集成和资源清理结果；不另造摘要或冻结清单 |

源码登记由 `contracts/maps/module-registry.json` 的 `dagpipe-runtime` 独占
`dagpipe/**`。该独立模块从已发布源码一次性迁入，原 `src/lib.rs` 为 3159 行；
模块合同将其源码行数上限固定为 3500，其他模块继续使用原有 1500 行上限。
`verify-sdk-source-registry` 必须核对唯一 owner 和模块上限；DAGpipe
独立测试是该模块的验证门禁。后续模块拆分可降低上限，但不得跳过登记门禁。

## 初始化及失败合同

`appsdk init` 负责项目治理初始化，Collab 身份与注册仍属独立控制面。
DAGpipe 的项目图由 AppSDK bundle 提供，不在每次项目 init 时做全局软件安装。
全局 `dagpipe` CLI、SDK 源码和 Skill 使用仓内独立安装入口。该入口必须使用本模块源码，
不访问旧 checkout；目标归属冲突显式失败。重复安装不能产生第二个 SDK 真源。
通用 DAGpipe 无常驻进程，其安装不启停 Collab daemon。

成功路径需覆盖 `dagpipe graph validate` 的有效图、无效图失败，`pipeline_runtime` 的公开 consumer，和 AppSDK 的现有图验证。失败路径包括 Cargo 编译失败、静态图无效和安装目标归属冲突。未跟踪源文件、旧独立仓库发布脚本、现有 Collab 身份/路由及其他人的 worktree 均在本次修改范围之外。

## 按改动选择验证

1. 依赖接入变化：确认 `rust/Cargo.toml` 使用仓内 `../dagpipe`，验证实际 consumer。
2. 引擎或 CLI 变化：覆盖受影响库行为、公开入口的有效/无效图及 AppSDK 接入。
3. 安装入口变化：在隔离目标验证 CLI/SDK/Skill 可用、归属冲突与重复安装行为。
4. 纯说明变化：检查语义与链接。未变图和有效测试证据直接复用。

构建、静态检查、安装和真实运行分别报告。普通改动不要求重装、源码哈希比对或发布流程。
