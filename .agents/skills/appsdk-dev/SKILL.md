---
name: appsdk-dev
description: "开发、调试、重构和发布 AppSDK 源码仓库；提供模块入口、风险选测、SDK 规则升级及正式安装操作。"
---

# AppSDK Development

## 从项目事实进入

读取 [根级 AGENTS.md](../../../AGENTS.md) 和当前任务笔记。
从 [module registry](../../../contracts/maps/module-registry.json) 找 owner，
按影响路径读取其余 maps 与设计。消费者项目的治理使用
`appsdk-project-governance`；本 Skill 负责 SDK 源码开发。
通用编码方法和 plan/review/交付按全局规则与实际宿主合同执行。

## 项目开发操作

1. 观察实际入口、当前行为与目标；debug 先尝试原样本或最小复现。
   开发/debug 将观察与受影响路径交独立 Planner；纯文档/Skill 编辑直接执行。
2. 项目代码从最新 `origin/main` 在
   `/Volumes/Intel/playground/appsdk/<task-slug>` 创建独占 worktree，分支用 `codex/`。
   定位合同 → 分派 → owner → 消费者/失败终点，不新建无关治理骨架。
3. 实现唯一 owner；按下方范围选测，并在 CLI/公开接口验证受影响成功、失败和副作用。
   改安装或运行入口时再补对应正式安装/live 验收。
4. 完整重大 milestone 自检与适用行为验收完成后，在提交/集成边界独立 review 一次。
   finding 修复只补受影响验证和 finding 复核。按已有授权交付并追加项目摘要；
   文档/Skill 编辑只做针对性检查，不扩大到代码测试、构建或发布。

## 按风险选测

下面是选择入口，不是每次全部执行的清单。命令从仓库根运行。
先从测试源码确认 target/过滤器覆盖实际用例；过滤结果为零不算通过。
优先复用仍绑定相同输入、产物和范围的有效证据。

| 变更范围 | 相关验证入口 |
|---|---|
| 根级规则、项目开发 Skill、普通文档 | 检查语义、引用路径、Skill frontmatter 和 diff；不执行 Rust 测试 |
| Guidance | `cargo test --manifest-path rust/Cargo.toml --locked --test cli_smoke guidance_`；涉及 setup/init 再选对应消费者用例 |
| Communication | `cargo test --manifest-path rust/Cargo.toml --locked --test communication_cli`；schema 变化再测 `communication_request_schema` / `communication_event_schema` |
| Memory | `cargo test --manifest-path rust/Cargo.toml --locked --test cli_smoke project_memory`；涉及独立 binary 再验证其公开入口 |
| Registry | `cargo test --manifest-path rust/Cargo.toml --locked --bin appsdk registr`；再选 registration/init 和受影响 Communication 消费者 |
| 初始化、迁移、pin、编译、生命周期 | 在 `rust/tests/cli_smoke/` 和 `rust/tests/sdk_*_migration.rs` 找实际受影响用例，选择 target/精确名称；跨模块时扩大消费者覆盖 |
| 合同、模板、SDK 分发 Skills | JSON/版本一致性；bundle 投影、已有项目规则保护、init/升级与相应 Guidance/Memory 消费者；安装行为变化再测 installer |
| DAGPipe | `cargo test --manifest-path dagpipe/Cargo.toml --locked --all-targets` 或其中受影响用例；再选 `rust/` 的 DAG 接入消费者 |
| Collab | `cargo test --manifest-path collab/Cargo.toml --locked --test <target> <filter>` 或对应 binary 单测；target 从 `collab/tests/` / Cargo 目标确认；live 投递变化按模块验证映射复测 |
| AppSDK installer | `bash scripts/tests/test-install-global-appsdk.sh`；再验正式入口的版本、安装字节与受影响消费者 |

局部实现先运行对应测试；共享入口、schema、依赖或未知影响需要说明扩大的风险依据。
扩大到某个 Cargo 包不等于运行整个仓库，也不自动触发 release build、安装或 daemon 操作。
Rust 格式检查使用对应 manifest 的 `cargo fmt -- --check`。
结构/owner/maps 变化时，使用适用候选 binary 的
`appsdk verify-sdk-source-registry .` 验证源码映射；该检查不替代行为验收。

### 当前 CI 的实际范围

[verify.yml](../../../.github/workflows/verify.yml) 是自动选测与 release 门禁的执行入口：

- 日常 push/PR 对 Guidance、Communication、Memory、Registry、资源、安装器、
  DAGPipe、Collab 和文档分别选择 job。
- 其余 `rust/src/*` / `rust/tests/*`、共享合同、版本/依赖及未知路径目前仍可能
  选择整个 AppSDK 包；workflow 变化或基线缺失会进一步扩大检查。
- 这是一项现存粒度限制。局部开发选测遵循上面的风险依据；修改 CI 选择器时，
  必须证明受影响消费者被覆盖，不能以空选择或绕 gate 缩短时间。
- 版本 tag 和 `workflow_dispatch` 触发完整 AppSDK release job。
  手动 dispatch 当前等同于请求完整发布候选检查。

## 规则与 Skill 升级

读取生效的全局规则、根级 AGENTS、项目开发 Skill、相关分发规则及实际 CI/hooks。
按差异记录位置、owner、delete/merge/narrow/add、依据、保留保障和入口影响。
直接复用当前任务笔记或审计文档，不新建强制 registry、审批账本或测试调度系统。

- 本项目架构/边界修改根级 AGENTS；开发操作修改本 Skill。
- 消费者模板修改 `templates/minimal/AGENTS.md`；分发 Skill 修改
  `sdk-skill-sources/<name>/`，通过正式 installer 安装，保留一个正文 owner。
- 分发升级用当前迁移机制追加版本，保留历史快照；审计消费者当前规则与 CI/hooks，
  按 owner 删除、合并或收窄旧规则，不能只刷新 SDK pin 就宣称完成项目规则升级。
- Guidance 仅在已选择使用且规则输入变化时重新 compile；SDK-only 升级不触发
  Collab 状态迁移、reset 或无关 daemon 维护。

## 构建、安装与发布

按改动涉及的产品选择正式入口：

| 产品 | 构建/安装入口 | 运行验收边界 |
|---|---|---|
| AppSDK | `cargo build --release --manifest-path rust/Cargo.toml --locked`；`scripts/install-global-appsdk.sh` | 官方 installer 安装两个 binary 和三个分发 Skills；验证 installed 版本及受影响真实消费者，无 daemon |
| DAGPipe | `scripts/install-global-dagpipe.sh` | 独立 CLI/SDK 安装；验证受影响图或真实库 consumer，无 daemon |
| Collab | `scripts/build-collab.sh`；`scripts/install-global-collab.sh` | 正式 release 构建由脚本分配 host build 版本；installer 不重启 daemon，按已授权模块维护合同执行官方 down/up 与 `collab context` |

候选 `target/` 产物用于 build/test；安装与 live 验收使用正式安装位置。
只按本任务授权操作相关 daemon；未知或共享 runtime 不自行 reset/重启。

完整 AppSDK release 门禁沿 `verify.yml` 的 release job：AppSDK 与 DAGPipe 完整测试、
格式、合同/版本、两个 release binary 构建、installer 事务、源码 registry 与真实
consumer smoke。Collab 不自动进入 AppSDK release 产品；受影响时走其单独门禁。

发布证据绑定准确候选与 commit/tag；公开资产下载后核对字节/哈希。
已发布 tag/assets 不修改。仅根级文档/开发 Skill 变化不升级 SDK 版本或重发产品。

### 平台与分发渠道

发布能力演进时，读取 [跨平台与渠道方案](../../../docs/design/release-distribution.md)。
按产品及 OS/CPU/ABI 定义验收；Windows 的路径、真实排他锁、事务、IPC 与进程
语义由对应 owner 处理，不能用无锁/空实现替代现有保障。各渠道复用已验 binary、
资源、版本与 commit；新增 npm 入口先验证 pack 和干净环境安装/升级，再公开发布。
同步 README 的起步命令与平台/渠道矩阵，只将已有适用证据的格子标为支持。
