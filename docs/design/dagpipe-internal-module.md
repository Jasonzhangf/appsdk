# DAGpipe 源码并入 AppSDK

## 目标与边界

将 DAGpipe `main` 的已跟踪源码（基线 `8cb59fb5262b5e1e440638faf3b04e06a2762cfb`）放入 AppSDK 的 `dagpipe/`，与 `collab/` 并列维护。`dagpipe/` 保持独立 Cargo package、`pipeline_runtime` 库和 `dagpipe` CLI。AppSDK 原有 `appsdk dagpipe` 命令继续负责 AppSDK 自己的 fix/notification Operator 和合同；它只依赖本仓库的 `dagpipe/` 库，不再从 Git 拉取同名库。通用图解析、编译、运行和静态 CLI 的源码 owner 唯一为 `dagpipe/`。

集成是一次性的源码归属迁移：从上述精确提交导入已跟踪文件，之后只在 AppSDK 主链开发和发布，不设置双向同步或独立仓库回写。DAGpipe 源仓库中未跟踪的 `.appsdk-control/` 和设计草稿不进入模块。现有项目图、AppSDK 专属 Operator、`appsdk dagpipe` 命令不搬入通用库。

## 业务语义 DAG

```mermaid
flowchart LR
  A[确认原始源码与现有调用] --> B[固定模块归属与版本]
  B --> C[迁入通用图引擎源码]
  C --> D[接通 AppSDK 本地依赖]
  D --> E[构建独立命令与 SDK]
  E --> F[验证现有图与真实调用]
  F --> G[统一主链发布]
  G --> H[回收本轮临时资源]
```

唯一入口是确认过的 DAGpipe 与 AppSDK 基线；唯一成功出口是本地和远端 AppSDK 主链一致、构建与调用证据齐全、临时资源回收。导入、编译、安装、调用失败均显式失败并保留候选，不跳到发布。取消时保留已提交设计及可审候选，按 owner 核销后才回收 worktree。

| 节点 | 唯一 owner | 输入 / 输出与证据 |
| --- | --- | --- |
| 确认源码 | 集成者 | 两仓 HEAD、dirty 状态、已跟踪文件与许可证元数据 |
| 迁入通用引擎 | `dagpipe/` | `pipeline_runtime` 公开 API 与独立 `dagpipe` CLI；原有测试及示例通过 |
| AppSDK 调用 | `rust/` | 本地 path 依赖；现有 `appsdk dagpipe validate`、fix 与 notification 入口可执行 |
| 安装初始化 | `dagpipe/` 安装入口 | 从本仓库构建并安装同源 CLI/SDK/Skill；第二次执行保持同一归属和结果，不创建第二份真源 |
| 发布与清理 | AppSDK 集成者 | 候选 SHA、产物哈希、review、主链和远端回执、worktree 不存在 |

## 初始化及失败合同

`appsdk init` 仍初始化项目治理与 Collab；DAGpipe 的项目图已经由 AppSDK bundle 提供，不在每次项目 init 时做全局软件安装。全局 `dagpipe` CLI、SDK 源码和 Skill 使用 AppSDK 仓库中的独立模块安装入口一次性安装或升级。安装入口必须使用本模块作为源码，不访问旧 DAGpipe checkout；已有安装位置只在确认归属后更新，冲突显式失败。重复安装不能叠加二进制或产生第二个 SDK 真源。安装不启停 Collab daemon，因为通用 DAGpipe 无常驻进程。

成功路径需覆盖 `dagpipe graph validate` 的有效图、无效图失败，`pipeline_runtime` 的公开 consumer，和 AppSDK 的现有图验证。失败路径包括 Cargo 编译失败、静态图无效和安装目标归属冲突。未跟踪源文件、旧独立仓库发布脚本、现有 Collab 身份/路由及其他人的 worktree 均在本次修改范围之外。

## 验收

1. `rust/Cargo.toml` 只引用仓内 `../dagpipe`；`cargo metadata` 的 `pipeline_runtime` 来源为该路径。
2. DAGpipe 库测试、独立 CLI 的有效/无效图、AppSDK DAGpipe 测试和真实命令入口通过。
3. 从已验候选安装独立 CLI/SDK/Skill，核对可执行文件及源码哈希；重复初始化结果可核验。
4. 独立架构 review PASS 后组合最新 `origin/main`，合入、推送并核对两端 SHA；仅清理本任务创建的 worktree 和临时资源。
