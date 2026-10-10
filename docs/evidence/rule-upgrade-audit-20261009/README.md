# AppSDK 0.1.0013 规则升级与架构审计

任务 `rule-upgrade-audit-20261009`，git-bug `aabed6a`。输入基线 `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`。用户需求见 [requirements.md](requirements.md)，独立计划见 [plan.md](plan.md)。

## 覆盖与证据边界

四份只读 observation 覆盖 AppSDK core/Guidance/registry、基础 communication/Memory/DAGpipe、Collab 与规则/模板/installer/CI。静态审计不会证明 runtime 正确。core 的六类生命周期入口已完成结构补查；内部部分分支语义仍列为 unknown，具体已读/未验证边界见各报告。

- [Core](core-report.md) 与 [覆盖补查详细笔记](coverage-notes.md)
- [Foundations](foundations-report.md)
- [Collab](runtime-report.md)
- [Rules and delivery](rules-report.md) 与 [详细笔记](rules-notes.md)
- [旧 pin 普通 init 正式 CLI 复现](upgrade-observation-report.md)

## 本次必要修改与保留

沿用 proposal、init、pin-lock、installer 和 review controller。规则升级要求读取生效上层规则、本地规则及实际测试/CI/hook，提出删除、合并或缩域，复用已覆盖会话授权，并按项目选择 Guidance。日常按风险选测，release 验收完整发布范围。普通 init 在写入前拒绝旧 pin，迁移继续由 pin-lock 管理。新增0012→0013迁移，历史快照不改写。

root contracts 和 .appsdk contracts 的消费者/用途不同，保留其职责。rules/Skill生成兼容路径保留，正文由SDK Skill source唯一维护。module-registry继续由项目维护，不改成SDK自动覆盖。没有新增审批账本、规则数据库或调度框架。

## 历史 advisory owner

- Collab owner：subagent CLI/MCP接线、迁移reset命令、authority文档与图、未消费legacy/migration实现。Collab是独立产品，本次没有安装、重启或发布它；静态发现须公开入口复现后另行处理。
- Communication owner：缺失root初始化与公开文档语义、reserved event兼容边界。
- Memory owner：schema版本层次、node-match声明与输出。没有消费者/目的和公开行为证据时不删除兼容面或补造功能。

开发定向、发布全套、安装、canonical consumer、正式review和远端release各有独立结果。最终记录由controller在相应动作完成后归档。当前文件是审计交付材料，不以文件存在声明release完成。最终作者验证见 [verification.md](verification.md)；正式review、main/remote和release回执在任务记录收口后归档。
