**DONE**
B 范围已完成，未 commit、merge、push、install、restart，也未修改 intake、Rust、版本、manifest、全局规则或 installed Skills。

主要结果：
- [templates/minimal/AGENTS.md](/Users/fanzhang/Documents/github/appsdk/templates/minimal/AGENTS.md:62) 增加升级审计规则：读取真实规则、测试命令和 CI/hook，复用已有授权，Guidance 可选，日常选测、release 全量。
- 三个 SDK Skills 均补齐适用边界。`appsdk-migration` 只用于真实控制面、daemon、身份或 reset 变化；SDK-only 升级不再套 Collab 迁移流程。
- 修正 `collab reset --project`、`appsdk reset-governance <project>` 等失效示例，并同步相关设计、架构和 reference 文档。
- 删除无消费者的 stale `templates/minimal/contracts/transitions/zone-transition-manifest.json` 和 orphan `contracts/transitions/zone-transition.schema.json`。
- 保留 canonical dot transition 合同、root 兼容 alias、project-owned `module-registry`。没有新增 bundle 资源。

验证：
- 三个 Skill 的 `quick_validate.py` 均返回 `Skill is valid!`
- `git diff --check` PASS
- `jq empty templates/minimal/.appsdk/project.json` PASS
- 删除路径无当前树悬空引用
- `patch.diff` 反向应用检查 PASS

交付物：
- [notes.md](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/source-rules/notes.md)
- [report.md](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/source-rules/report.md)
- [patch.diff](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/source-rules/patch.diff)，SHA-256 `58b234af7a05e016c5ac914d2b7afd9dc8e9d4083fcf3fbcdeb2bdd164d6b3f4`

未验证边界：未运行 Cargo 测试、build、installer、安装、daemon 或 runtime replay。组合树测试、安装和 release 验证由 controller 统一执行。