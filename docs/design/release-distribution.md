# AppSDK 跨平台与发布渠道方案

状态：发布要求与实施方案。README 入口已整理；平台 port、产物矩阵与 npm
包装尚待实现。本文区分当前事实与计划，不作为新增平台支持的验收记录。

## 发布目标与产品边界

用户要求：根 README 包含介绍、文档路由、治理起步、三个模块的关系与操作；
考虑 Windows/Linux/macOS 适配；考虑 npm 等发布渠道。

AppSDK、DAGPipe、Collab 继续保留独立源码/安装/运行 owner。
AppSDK 分发 `appsdk`、`project-memory` 与三个 SDK Skills；DAGPipe 分发
CLI、Rust SDK 与运行 Skill；Collab 分发两个 binary 与协作 Skill。
同仓库维护和统一导航不将全部模块变成强制 runtime 依赖。

## 当前发布基线

| 范围 | 当前证据/缺口 |
|---|---|
| `v0.1.0014` | GitHub 公开资产为 macOS ARM64 的 AppSDK、project-memory、Skills tgz 和 SHA256SUMS |
| Linux x64 | Ubuntu release job 完成 AppSDK/DAGPipe 测试、构建、安装器和消费者 smoke；没有该 release 的 Linux 二进制资产 |
| macOS ARM64 | AppSDK canonical installer/消费者及公开下载资产已有证据；Collab 的独立安装/live 记录不属于 AppSDK release 产品 |
| 其他 CPU 架构 | 当前发布未提供独立平台资产及完整验收；不能从同 OS 的另一架构推广支持结论 |
| Windows | 只有部分 HOME/USERPROFILE 路径兼容代码；主要产品存在待适配项，未做本轮 Windows 编译或 runtime 验收 |
| npm | 主包已确定为 `@jsonstudio/appsdk`；仓库没有 package.json、launcher、平台包或 npm 发布 workflow；现有 registry 版本与账号权限需核对 |

2026-10-10 对公开 npm registry 查询 `@jsonstudio/appsdk` 返回 `E404 Not Found`。
这只表明本次未取得公开包元数据，不证明 scope 归属、账号写权限或名称可注册。

已发布版本资产见 [v0.1.0014](https://github.com/Jasonzhangf/appsdk/releases/tag/v0.1.0014)。
现有 release gate 见 [verify.yml](../../.github/workflows/verify.yml)；
源码审计证据来自下列 owner，静态检查不宣称 Windows 运行失败已复现或已修复。

## 平台适配的最小范围

首轮建议目标为 macOS ARM64/x64、Linux x64 GNU、Windows x64 MSVC。
Linux ARM64、Windows ARM64、musl 属于后续可按需求加入的 target；
每个 target 写明最低系统/运行库版本，不以构建 runner 的名字代替兼容承诺。
WSL 归 Linux 验收，不能算原生 Windows。

### AppSDK

- 路径与安装：当前 `scripts/install-global-appsdk.sh` 使用 Bash、HOME、
  Unix 可执行权限、symlink 和无 `.exe` 的目标名。需要原生 Windows 入口、
  user-home/安装根、PATH 与可执行名处理，保留同一安装/升级语义。
- 文件锁：`rust/src/global_registry.rs` 只在 Unix 分支取得 flock；
  `rust/src/main/longhorizon.rs` 的非 Unix advisory-lock 分支直接成功。
  Windows 必须建立真实排他锁；不能将无锁运行宣传为等价并发保证。
- Reset 事务：`rust/src/main/reset_transaction.rs` 的 symlink-component
  helper 仅定义于 Unix，却在事务路径中调用。需处理编译边界与 Windows
  路径/reparse-point 语义，保留路径隔离、失败恢复和原子替换保障。
- 测试/消费者：将确需 shell 的 fixture 与纯合同用例分开，原生平台用真实
  CLI 验证 new、prepare/init、升级、规则保留、锁竞争和失败后的数据保护。

### DAGPipe

`dagpipe/` 的 Rust 核心是独立确定性库；CLI 已处理 HOME/USERPROFILE，
但 `dagpipe/scripts/install.sh` 仍依赖 shell、Unix 文件工具和安装树操作。
需验收三个 OS 的库 consumer、CLI 图校验、SDK path 与安装升级。
Windows 的 SDK 路径必须能直接用于 Cargo manifest；不把格式正确的路径
当作可消费 SDK 的证据。

### Collab

- `collab/src/client.rs`、`collab/src/server/mod.rs` 与
  `collab/src/adapters/codex_app_server_production_part*.rs` 使用 Unix stream/listener。
  Windows daemon IPC 与宿主 App Server transport 是两个独立边界；
  根据真实宿主能力选择平台实现，保持请求、身份和回执合同。
- daemon 文件锁、权限、进程启动/退出、PID 与状态根也要适配。
  `collab/scripts/build-collab.sh` 依赖 flock/lockf；installer 使用 shell/Unix 工具。
- 平台代码收敛到相关 owner 的最小边界。无 transport 能力时显式报告
  unsupported/unavailable，不能通过临时 TCP、tmux 或伪造投递回执变绿。
- 原生支持的终点是受控 daemon 生命周期、身份恢复、持久 send/recv 与真实
  宿主消费回执。daemon 启动成功或 IPC 单测不能替代该证据。

## 从候选到渠道

```text
准确源码候选 + 固定产品版本
  → 各 target 构建与受影响平台验收
  → 平台 binary + Skills/SDK 资源 + 哈希
  → 对应渠道打包
  → 干净用户环境安装/升级与公开入口验收
  → 独立 milestone review
  → 不可变源码 tag、GitHub 资产与渠道版本
  → 下载/安装回执与 README 支持矩阵
```

每个发布产物绑定产品、版本、OS、CPU、ABI、源码 commit 与哈希。
优先沿现有 bundle/版本资源扩展，不另建规则数据库或第二套发布账本。
同一 target 的已验 binary 复用于 GitHub 与 npm，不按渠道重新编译。
macOS 需明确签名/notarization 状态；各平台依赖与权限要求在下载/安装说明中公开。

Collab 当前正式构建从 host build counter 分配版本。多 target 发布需要在其
正式 build owner 中一次分配并固定版本，再传给所有 target；不能每个 runner
各自递增后将不同版本当作同一 release。该机制修改须独立计划和受影响验收。

## npm 渠道设计

用户已确定 npm 主包为 **`@jsonstudio/appsdk`**。首发先交付 AppSDK 产品，
DAGPipe 与 Collab 是否增加独立 npm 包按后续需求确定，保持三模块 owner 边界。
目前尚无本仓库的可执行 npm 安装命令。pnpm/yarn 可消费同一 npm registry
产物，无需另建一套发布实现。

- npm 包提供小型 Node launcher 与明确的 `bin` 映射，核心实现仍是 Rust。
  AppSDK 包导出 `appsdk`/`project-memory`；DAGPipe 导出 `dagpipe`；
  Collab 导出 `collab`/`collab-mcp`。
- 默认采用按 OS/CPU/ABI 限定的 optional platform packages，将既有已验 binary
  随 registry tarball 分发；launcher 只解析本机产物并透传参数、退出码和信号。
  不在运行时从 latest 下载二进制，不静默从源码构建或切换未知平台。
- 明确最低 Node 版本；optionalDependencies 被禁用、缺包、平台不支持和版本
  不匹配须给出具体错误。安装自身不启动 daemon、不初始化项目或改身份。
- Skills 与 DAGPipe SDK 随产品资源分发。写入共享 Skill 根及 canonical 安装
  位置由显式安装/setup 操作调用同一产品 owner；避免 npm postinstall 自行
  覆盖其他渠道的文件。新入口须定义升级、卸载和混装时唯一实际 binary。
- `0.1.0014` 含前导零，不能直接作为 npm SemVer。沿现有 Cargo 版本建立明确
  映射，例如 AppSDK `0.1.0014` → npm `0.1.14`，保留原 SDK 版本与 commit 关联。
  Collab 同理；先检查版本唯一性再发布，不另造不一致版本链。
- 发布前验收 npm pack 内容与本地 tarball 安装、全局 bin、npx、路径含空格、
  升级/卸载和真实消费者。从 registry 下载的包再核对完整性与版本回执。
- npm 账号权限、包名、2FA/可信发布与 provenance 在实际 publishing workflow
  设计时核对；不保存 token 到仓库或任务笔记。

GitHub Releases 继续作为源码 tag、平台下载与哈希入口。Homebrew、winget、
Scoop、apt 等后续按用户需求加入，只消费已验产物；当前不建立全部渠道。

## 交付增量与验收

| 增量 | 最小交付 | 完成依据 |
|---|---|---|
| 文档入口（本轮） | README 介绍/路由/治理起步/三模块操作、平台和渠道事实；本方案 | 入口与实际 CLI/脚本一致，引用和文档 diff 检查 |
| 平台基础 | AppSDK/DAGPipe 原生 Windows 边界及三 OS 构建/安装入口 | 每 target 的公开 CLI/真实库消费者、升级与失败数据保护；缺证据的格子仍标未验证 |
| Collab 平台支持 | IPC/宿主 transport、锁和受控进程生命周期 | 每声明支持平台的真实 native 消息消费链；能力不足只阻断 Collab 支持声明 |
| 产品资产 | 固定版本/commit 的 target 产物与资源 | 正式构建、渠道共用字节、hash、干净环境安装；已发布历史保持不可变 |
| npm 首发 | 用户包、platform packages、launcher、资源安装与 publishing workflow | pack/本地安装/公开入口验收、独立整体 review、registry 回执 |

未来代码实现前按项目 Skill 将本次事实交独立 Planner；每个完整重大交付
milestone 验收后独立 review 一次。日常按受影响产品/平台验证；完整矩阵在
release 执行。纯文档修改保持文档检查，不能因本方案存在就每次全仓回归。

未确定项：npm 已有版本与发布账号权限、各 OS 最低版本、Linux ABI、Windows 宿主真实
transport 能力、各平台 runner、签名与发布账号配置。实现时只前置阻断当前
交付单元的未知；不因此拖住已有支持平台的文档和治理入口。
