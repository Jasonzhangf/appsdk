你是独立只读架构reviewer，作者已完成debug、开发测试与公开E2E。按 /Users/fanzhang/.agents/skills/codex-review/review-standards.md 与 review-prompt.md 执行；不修改代码，不补测试，不替作者排障。当前mode=uncommitted，scope.commit回显HEAD 8555a74e5e46de10136fda9539ae044d3e8bdfbc，base=8555a74e5e46de10136fda9539ae044d3e8bdfbc。精确已验候选staged Git tree=0251a454b672d850540323135ffb55603b4c2364。读git diff HEAD，包含新文件。生产代码冻结，后续仅保存审查/交付文档证据。

权威输入来自本会话真实用户原文，来源为当前root用户消息，完整转录在 docs/design/user-requirement-truth-lock.md第1节和docs/design/requirement-session-contract.md：
1 “检查我们现在是否有机制 lock 项目的真相不让更改？”
2 “用户可能需要输入一些需求，只能用户提出修改才能修改，否则永远作为真理存在”
3 “你画一个 dag 图，以及现在的 gap，落盘实现的文档，然后给我 goal 提示词”
4 “我认为权威要求是我们 reviewer 必须要校验的部分，所以需要加入 reviewer 的完整提示词要求中”
5 “这个应该是 appsdk 治理的一部分，不仅仅是我们现在使用的 reviewer 提示词，需要 agent 去根据要求把 reivew 模板加入我们的这部分真相”
6 “你现在要使用 gcm worker 进行编排执行”
7 最新明确修改：“用户在会话里提供授权就行了，你不要做无用的校验，我们不要过度校验”。

最新用户指令替代旧设计中过强认证前提。真实会话原文和source就是授权；执行agent转录，reviewer核对具体范围。不得把签名、身份认证、外部服务、物理权限隔离或恶意同UID整套文件伪造防护重新作为门禁。机器只检查必要结构、项目/条目/基准、原子提交及消费绑定。不要检查无关身份恢复/Collab注册或要求源repo初始化。

当前合同：requirements.rs唯一owner，项目.appsdk/requirements.json单一原子ledger保存原始请求/版本历史，当前条目从历史派生；公开CLI show/history/apply/verify，create/replace/revoke/cancel，缺会话原文/来源拒绝，幂等重放、冲突和并发只一次提交；goal.requirements_version引用当前总版，compile/verify/review/lifecycle/Guidance现有goal消费边界共用owner；旧项目无锁兼容明确not_established；init/reset/fresh-init/SDK pin升级保留ledger；SDK自有正式review模板分发、review-context加载全部需求/历史及来源并复用ReviewRecord.context_id绑定。不是只改全局提示词。

必读审查材料：
- docs/design/requirement-session-contract.md（冻结当前合同）；
- docs/design/user-requirement-truth-lock.md（G1-G9原始gap、T1-T16当前适用验收）；
- docs/goals/user-requirement-truth-lock-goal.md（已落盘）；
- 两份docs/dagpipe/user-requirement-*.graph.json：静态SESE责任映射，不新增Runtime Operators；
- SDK唯一模板sdk-skill-sources/appsdk-project-governance/references/authoritative-review-template.md及review_context/registry/guidance/reset/requirements实际接线；
- docs/evidence/user-requirement-truth-lock-20261007/session-lock-notes.md；
- session-lock/delivery.md及author/validation.json、rust-test.log.gz、dagpipe-test.log.gz、installed-blackbox.log.gz、installed-pin-blackbox.log.gz、install.log、source-registry.json、embedded-dag.json；
- 同目录design-review/status.json/final.json/exit：独立设计controller pass，不重复设计准入。

作者证据：完整cargo test Rust exit0（40 unit、351 CLI、6 communication、99 memory、5 parser、20 reviewstore、7+1旧SDK、2迁移）；DAGpipe --locked --all-targets exit0/28；11组需求真实CLI、installed11组通过；最终pin历史恢复修正后canonical受影响升级37组exit0。官方scripts/install-global-appsdk.sh已安装0.1.0012及同源Skills，canonical/release SHA256 d8470cd7ddf6ca0833b6d7a780781c9a1c0f5ec2c9809702929c9f31a7023fbb。没有daemon行为修改，不适用daemon重启。Collab仅机械拆分超限测试文件，原global_state38通过，无daemon安装要求。source registry、JSON、版本一致性、Rust/DAGpipe fmt和graph验证通过。

逐条对照权威需求、设计、diff和真实外部行为。核验授权修改范围以及既有需求持续性；未授权改需求/降验收/缺必需版本或来源按P1，严重伪造按P0；自然语言授权检查归agent/reviewer，不用机器结构字段冒充身份认证。保留来源和当前版本到既有module_boundary_evidence resources/edges/gates，不新增schema。候选安装已完成；review后CI/merge/push/资源清理是下一生命周期节点，不把尚未允许执行的下游集成当作者准入缺失。不要重开已经有效且未变的检查。

只返回supplied JSON schema：contract_version="1"、scope={mode:"uncommitted",commit:"8555a74e5e46de10136fda9539ae044d3e8bdfbc",base:"8555a74e5e46de10136fda9539ae044d3e8bdfbc"}、module_boundary_evidence、findings；controller负责PASS/FAIL。
