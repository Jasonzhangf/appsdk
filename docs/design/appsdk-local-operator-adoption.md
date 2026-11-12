# AppSDK 本地算子化采用

本项目采用已分发的骨架、Operation、节点与算子迁移流程。具体方法由治理
Skill 的正文维护。项目模块、路径、符号和 owner 继续由根级 AGENTS 与
module registry 承载。本文记录本项目的实际采用边界和首轮验收。

- [通用采用流程](../../sdk-skill-sources/appsdk-project-governance/references/bootstrap-migration.md#skeleton-operations-and-operators)
- [全项目架构骨架](../../AGENTS.md#产品与架构)
- [模块与能力 owner](../../contracts/maps/module-registry.json)

## 现有组合与本轮位置

| Operation 或能力 | 配置及真实消费者 | 本地采用位置 |
| --- | --- | --- |
| notification 对象检查 | `contracts/dagpipe/notification.graph.json`，`dagpipe_notification.rs` 注册并执行 | 已有配置组合入口 |
| fix 生命周期投影 | `contracts/dagpipe/fix-lifecycle.graph.json`，`dagpipe.rs::run_fix_lifecycle` | 已有辅助投影入口，结果保留其 advisory 权威范围 |
| verify 基础合同检查 | `contracts/dagpipe/verify-foundation.graph.json`，`promotion.rs::verify_internal` | 本轮连续迁移段 |
| verify 测试证据、模块 review 与发布余项 | `test_governance.rs`、`review_gates.rs` 及其现有 owner | 保留各自合同，后续按 Operation 细化 |
| Guidance、Communication、Memory、Registry、Collab、资源安装 | 各模块公开入口及 module registry | 后续依真实使用路径选择增量 |

每条业务路径的骨架从公开入口闭合到结果、失败和必要副作用。采用状态随实际
接线和验收更新。设计图的登记和真实运行接线分别有自己的证据。

## 首轮 Operation

公开入口是 `appsdk verify`。本轮组合范围是安全读取、maps 结构、声明合同和
项目合同检查。`verify_internal` 取得图的 `project_checked.project`，再执行
原有余项。基础段的业务行为为只读，公开命令边界保留一次调用日志。

| 节点及算子 | 输入与输出 | 能力 owner | 失败边界 |
| --- | --- | --- | --- |
| `load_project` / `appsdk.verify.load_project@1` | `Null` → 包含原项目值的 Object | project | 根路径、读取及 JSON 解析错误 |
| `governance_baseline` / `appsdk.verify.governance_baseline@1` | 项目 Object → 同一业务值 | registry | maps 缺失、结构和必要字段错误 |
| `declared_contracts` / `appsdk.verify.declared_contracts@1` | 项目 Object → 同一业务值 | governance | 声明集合、transition 和 record schema 语义错误 |
| `project_contract` / `appsdk.verify.project_contract@1` | 项目 Object → 同一业务值 | project 及现有能力 owner | 项目、模块、归属、表面及构建合同错误 |

`verify_foundation.rs` 负责算子适配、版本注册和图执行。验证实现留在对应能力
owner 的返回式核心。现有调用者通过薄适配保留各自的错误出口。

## 关系、状态和执行流

一次调用持有自己的 typed invocation。项目值沿 ARC 传递，根路径和控制状态
由调用资源提供。每个节点绑定一个版本算子，图终点交回唯一消费者。

```text
接收校验请求
  → 准备根路径与调用状态
  → 读取项目声明
  → 检查治理映射
  → 检查声明合同
  → 检查项目合同
  → 返回已检查项目
  → 执行后续校验
  → 输出公开结果

各节点校验失败 → 返回业务错误 → 命令失败出口
```

基础图采用串行依赖，入口和终点各一个。内置 JSON 声明节点顺序、ARC、类型和
算子版本。每次调用注册并编译所选配置，基础段保留原诊断和首错顺序。

## 迁移及验收边界

本轮验收按独立能力、组合和公开入口推进。能力验证覆盖合法输入、专属错误和
失败后继续处理下一次调用。组合检查复用 DAGPipe 的类型与绑定检查。

新旧比较沿相同输入核对业务结果、错误顺序和必要副作用。已有 compiled 比较
使用摘要消融后的旧过程式基线，其来源与适配范围保留在任务证据中。公开
日志新增的调用记录单独核对。切换后基础段的旧接线及失效专属检查随唯一实现
收口，保留有独立保障的安全与数据正确性检查。

本轮公开成功验收覆盖 draft 和 compiled 的 test-governance-off 路径。
test-governance-selected 输入的基础段错误可在身份查询之前验证。selected
成功分支的证据适用性、历史 review 和发布余项各自保留待迁移边界。

强制治理条款的审批、可执行绑定及观测沿 ontology 模板处理。当前设计与事实
记录用于指导这次本地采用。[Ontology 模板](../../sdk-skill-sources/appsdk-project-governance/references/ontology-governance.md)
