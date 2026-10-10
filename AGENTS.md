# AppSDK 项目规则

## 入口与归属

本文件保存 AppSDK 源码仓库的架构、功能和边界。开发、debug、重构、
规则升级和发布时，读取项目 [appsdk-dev Skill](.agents/skills/appsdk-dev/SKILL.md)。
通用方法、独立 plan/review、授权和资源管理沿用生效的全局规则与宿主编排合同；
项目 Skill 只补充实际入口和验收，不建立第二套生命周期。

- 架构、功能边界 → 本文件；开发命令、选测和交付 → `appsdk-dev`。
- 机器合同与 owner → `contracts/`；设计理由 → `docs/design/`。
- 当前任务事实与证据 → 独占任务笔记；项目摘要 → 单 owner 追加 `note.md`。
- 对消费者分发的规则 → `templates/minimal/AGENTS.md`；SDK 分发 Skills →
  `sdk-skill-sources/`。它们与本项目开发 Skill 的作用域不同。

## 产品与架构

AppSDK 提供通用项目骨架、治理合同、确定性编译和证据校验。
消费者项目拥有业务语义、provider、业务协议和 pipeline；SDK 提供机制。
仓库包含三个独立 Cargo package，执行命令须指定对应 manifest。

| 单元 | 唯一源码入口 | 职责与依赖 |
|---|---|---|
| AppSDK | `rust/src/main.rs`、`rust/src/main/` | `appsdk` CLI：初始化、迁移、合同验证、编译、promotion/freeze/publish、需求与交付证据；主入口分派到各 owner |
| Guidance | `rust/src/guidance.rs`、`rust/src/guidance/` | 编译声明规则，校验并记录 Plan/Revision/Step，投影下一步；不调用模型，默认辅助 |
| Communication | `rust/src/communication.rs`、`rust/src/communication/` | `appsdk-comm/v1` 请求、事件、adapter、投递与消费记录；队列接受与实际执行分开 |
| Registry / long horizon | `rust/src/global_registry*.rs`、`rust/src/long_horizon_*.rs`、`rust/src/main/longhorizon.rs` | 项目/runtime 注册与长期任务控制；身份与通信事实依各自 owner |
| Project Memory | `rust/src/memory.rs`、`rust/src/memory_cli.rs`、`rust/src/bin/project-memory.rs` | 本地记忆索引、查询与显式 review/promote；独立 `project-memory` binary，共用实现 |
| DAGPipe | `dagpipe/src/lib.rs`、`dagpipe/src/bin/dagpipe.rs` | `pipeline_runtime` 库与独立 `dagpipe` CLI；`rust/` 以 path dependency 使用；AppSDK 专用接入在 `rust/src/dagpipe*.rs` |
| Collab | `collab/src/`、`collab/skills/collab/` | 独立 `collab` / `collab-mcp` binary、daemon、身份、mailbox、协作与资源归属 |
| 资源与安装 | `contracts/`、`templates/minimal/`、`sdk-skill-sources/`、`scripts/` | bundle、schema、迁移历史、项目骨架、分发 Skills 与各产品正式安装入口 |

主要路径：

```text
消费者 CLI 输入 → AppSDK 命令分派 → 对应合同/实现 owner → 校验/产物/明确错误
合同 + 模板 + 分发 Skills → bundle/正式 installer → 消费者初始化与升级
AppSDK 图命令 → AppSDK DAG 接入 → pipeline_runtime
协作请求 → Collab CLI/daemon → 身份与 durable mailbox → 宿主投递/消费回执
```

详细 owner、符号、资源和验证映射以
[module registry](contracts/maps/module-registry.json) 及 `contracts/maps/` 为准。
只有受影响的 owner、路径、符号、依赖或验证绑定变化时，更新对应 map。
复用 `docs/dagpipe/` 中适用的图；文档编辑和拓扑未变的局部修复不新建图。

## 功能与合同导航

按任务读取相应设计，不全量加载：

- 初始化、模块绑定与 SDK 集成：[项目集成](docs/design/appsdk-project-integration.md)。
- 编译、promotion、freeze、Active/Protected 与生命周期证据：
  [promotion 合同](docs/design/playground-active-promotion.md)、
  [生命周期记录](docs/design/lifecycle-records.md)、
  [消费者治理架构](docs/architecture/appsdk-governance-architecture.md)。
- Guidance：[Harness 架构](docs/architecture/development-process-control-harness.md)、
  [详细合同](docs/design/appsdk-guidance-framework.md)。
- Communication：[通信接口](docs/design/apps-sdk-communication.md)。
- DAGPipe：[模块职责](docs/design/dagpipe-internal-module.md)、[运行库入口](dagpipe/README.md)。
- Collab：[模块入口](collab/README.md)、
  [验证映射](collab/docs/verification-map.md)、
  [原生投递架构](docs/codex-tui-collab-architecture.md)。

## 源码、消费者与控制状态边界

本根目录默认是 SDK 源码与发布仓库。创建本文件和开发 Skill 不等于注册
AppSDK 消费者合同。将本仓库纳入消费者治理时，按
`appsdk-project-governance` 的 prepare/确认/init 流程，显式绑定源码模块。

消费者的 `.appsdk/` 保存项目合同与记录，`.appsdk-control/` 保存本地运行态；
Playground/Active/Protected/generated 是消费者治理分区。不能据此把本仓库
`rust/`、`dagpipe/`、`collab/` 当作生成物或冻结副本。

- AppSDK 全局项目/runtime/communication 真源：`~/.appsdk/` 的声明注册资源。
- Collab 全局身份、route、mailbox 与 daemon 真源：`~/.collab/`，由 Collab 控制入口管理。
- `.agent-collab/` 为项目局部状态；存在目录不证明当前 peer 已注册。
- 只修改所属产品的源码与控制入口。SDK 文档/规则升级不触发 Collab reset、
  身份迁移或 daemon 重启；协作准入与质量准入分开。

## 演进与验收边界

- 采用最小充分结构、最小必要流程；沿实际依赖修唯一 owner，并消融范围内已确认冗余。
- 日常变更只做受影响检查及必要消费者验证；共享合同或依赖的风险决定扩大范围。
  release 执行声明的完整门禁。实际 CI 选择规则见开发 Skill。
- 新证据只更新受影响结论；实质性的目标、范围、验收、关键方案或依赖变化才重新规划。
  review 触发遵循宿主重大 milestone 合同；文档/Skill 使用针对性检查。
- Guidance、Memory 和未启用的 Collab 属于辅助能力；不将所有可用命令列为每次必经步骤。
- 历史迁移快照、已发布 tag/assets 保持不可变；版本演进使用现有迁移机制追加。
  版本来源是 `rust/release-version`、`rust/build.rs`、bundle 与模板 pin，按真实依赖同步。
- AppSDK release 产品包含 `appsdk`、`project-memory` 与三个 SDK-managed Skills。
  DAGPipe 参与发布门禁，使用独立 installer；Collab 使用独立构建、安装与维护流程。
- 文件存在、`verify` 成功或 review PASS 分别只证明对应范围；报告源码、安装、
  live/消费者、远端、发布与资源清理的实际状态，不扩大证据含义。

## 对外发布入口

- 根 README 保存产品介绍、文档路由、治理起步、三模块关系与实际安装操作。
- 平台支持按产品、OS、CPU/ABI 及真实安装/运行证据声明；源码可构建、CI 通过、
  下载产物和 live 支持分别报告。WSL 不等于原生 Windows。
- 渠道复用相同源码版本和已验产物；npm 等包装不复制核心实现，也不隐式接管
  daemon、身份或项目初始化。当前状态和后续实施见
  [跨平台与发布渠道方案](docs/design/release-distribution.md)。
