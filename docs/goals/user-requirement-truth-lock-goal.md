# 用户需求真相锁 goal 提示词

本文件是完整目标的可复制执行提示词。用户随后已要求使用新建 GCM worker 执行。当前实现增量见 `docs/design/appsdk-authoritative-review-packet.md`：先交付 SDK 模板分发、公开上下文组装和 review 绑定；完整需求锁的其余验收仍有效。没有创建 goal 自动订阅。

```text
/goal
身份：你是本目标编排者。Codex Desktop/TUI 使用 codex-orchestrator；DSH 使用 dsh-create。负责拆分、派单、验收与自有资源回收，不自动获得 Collab master 身份。

目标：在 AppSDK 中实现用户需求真相锁。已确立的用户需求持续有效，只有真实用户明确提出的指定变更才能修改、替代或撤销。Agent 可以建议和修改实现，不能自行改需求或降低验收。用户合法变更产生新版本，保留原文、旧版本和授权来源。

仓库：/Users/fanzhang/Documents/github/appsdk。
实现依据：先读 docs/design/user-requirement-truth-lock.md、docs/design/appsdk-authoritative-review-packet.md、docs/dagpipe/user-requirement-change.graph.json、docs/dagpipe/user-requirement-consumption.graph.json，以及 docs/evidence/user-requirement-truth-lock-20261007/notes.md。文档的 G1–G9、T1–T16、owner、边界和停止条件是本目标合同。G9/T16增量已有独立DESIGN_PASS及代码/作者验证；完整G1–G8需求锁仍仅有拓扑设计，须补能力证据与独立设计准入。先读最新交付记录，复用已有效证据，不重复实现已交付增量。

编码前：从工具读取当前路径和规则，核对最新源码与真实调用链。先证明真实用户授权入口、授权与 agent 权限隔离、公开 consumer/harness 和交付能力。完善 SESE DAG，并取得独立设计 review PASS，之后才写产品代码。不得以自填 confirmed_by、消息指针或 agent 可重算哈希冒充用户授权；必需能力缺失时报告具体 blocker，不以弱化方案宣称需求已锁定。

范围与约束：复用目标澄清、治理、提交与消费 owner；保持需求变更与任务消费两条独立闭环。保护长期需求与历史，不因任务关闭、重启、SDK 升级或治理 reset 失效。不得自动初始化本源码仓库、修改其他项目、扩大需求变更权限、重启无关 daemon 或新增重复真源。开发代码从最新 origin/main 建立独占 /Volumes/Intel/playground/appsdk/<task-slug> worktree；外置盘或 base 不可用时停止。现有设计文档的未提交改动是已授权交接产物，先核对并保留，在候选中按 owner 纳入，不覆盖或删除。

开发节奏：开工前划定当前可用增量。先完成一条真实主线：用户确立需求 → 可信授权核验 → 持久化版本 → agent 只读消费 → 无授权修改拒绝 → 用户合法修改生效 → 公开入口验收。随后按依赖完成保留、兼容、受影响 gate 和 SDK 分发，总验收不降低。阻断授权、安全、数据正确性或当前验收的问题立即处理，其余不打断主线。

执行：优先可通信且获授权的 peer，其次宿主支持的独立 worker；没有可用执行者时独立推进。Codex Desktop worker 按 Skill 使用新建 codex exec --profile gcm，不用内置并行入口。依赖独立且所有权不重叠的任务并行派发，实现与独立审查分开。未注册 Collab 不寻找或自建 Master。每个节点的结论和证据写入独占 run notes，按有效笔记推进。

reviewer 合同：这是 AppSDK 治理能力。必须在 SDK 自有真源加入正式 review 模板和 bundle 分发；执行 agent 通过 AppSDK 公开入口加载项目权威需求/版本，并组装候选/base、变更授权和验收证据。AppSDK 校验完整材料，再交给独立 reviewer，最后核验审查结果与同一需求/候选的绑定。不能只改当前 Codex/AGY 全局提示词。需求内容不得由 agent 任意填写；未授权改需求、降验收、撤销或必需来源缺失必须阻断。按文档 7.1、7.2 和 T16 从新建真实消费项目验证模板分发、组装、review 准入及记录闭环。

验收与交付：执行文档 T1–T16 的全部适用公开入口黑盒场景，覆盖合法、伪造、无授权、版本冲突、重复/并发、写入失败、取消、reset 保留、陈旧消费和 reviewer 核验。记录精确候选、输入/环境、公开结果及副作用。作者开发测试与 E2E 全部通过后取得独立架构 review PASS。沿全局/项目已授权交付闭环完成最新 main 组合、适用构建安装及运行验证、CI、clean main 集成、push 回执和自有资源回收。不得用图验证、单测、源码结构或成功格式的字段替代行为证据。

完成标准：文档全部适用验收已证实，唯一需求 owner 和可信授权边界成立，需求版本持久、消费链和保留链接通，适用工程交付与资源回收完成。未满足时报告 INCOMPLETE 或 UNVERIFIED、具体缺口及下一步。

直接执行本任务，不再为它生成另一层提示词；不自动创建订阅。
```
