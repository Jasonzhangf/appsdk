# 用户需求真相锁 goal 提示词

先读当前交付回执和节点笔记，复用已完成的实现与有效证据。此提示词不创建自动订阅。

```text
/goal
你是本目标编排者，负责拆分、派单、验收和自有资源回收。使用 codex-orchestrator；需要 worker 时只新建 codex exec --profile gcm，不使用内置并行入口。无 Collab 注册时独立执行，不建立虚构 Master。

仓库：/Users/fanzhang/Documents/github/appsdk。
先读：docs/evidence/user-requirement-truth-lock-20261007/session-lock-notes.md、session-lock/delivery.md、docs/design/requirement-session-contract.md、docs/design/user-requirement-truth-lock.md，以及 docs/dagpipe/user-requirement-change.graph.json 和 user-requirement-consumption.graph.json。

目标：完成并交付 AppSDK 用户需求真相锁。用户明确确立的需求持续生效，只有用户在会话中明确提出的指定变更才能修改、替代或撤销。真实会话指令就是授权，执行 agent 保留原文及 source，reviewer 对照原会话核验范围。不得追加身份认证、签名、外部服务或物理权限隔离。

唯一 owner 是 rust/src/main/requirements.rs，唯一账本是项目 .appsdk/requirements.json。复用原子写和 reset 锁。保留全部原始请求与逐条历史；当前需求从历史派生，不新增镜像、数据库或哈希链。公开入口为 appsdk requirements show/history/apply/verify。goal 只引用 requirements_version。普通升级、reset 和 fresh init 必须保留账本。旧项目未建立锁时保持兼容，状态明确返回 not_established。

review 是 AppSDK 治理能力。复用 SDK 自有 authoritative-review-template.md、公开 review-context 和既有 ReviewRecord 绑定。完整提示词必须加载全部适用权威需求、原文、版本、历史、授权来源、精确候选及作者行为证据。reviewer 逐条核验，不能自行批准需求变化或降低验收。不得以当前机器的全局 reviewer 提示词替代产品模板。

先按节点笔记核对当前状态，只补尚未完成或已失效的节点。依赖独立且文件 owner 不重叠的任务并发派给 GCM worker。保护共享根的其他 owner 改动；代码只在最新 origin/main 的 /Volumes/Intel/playground/appsdk/<task-slug> 独占 worktree 中修改。

验收：执行设计中的全部适用 T1–T16，通过真实 CLI/公开 consumer 验证原文与历史、会话授权提交、缺字段拒绝、指定条目/旧版本冲突、幂等重放、并发、写失败、取消/撤销、升级/reset 保留、陈旧消费、正式 review 材料与绑定。机器只做必要结构、版本和契约一致性检查，不承诺识别同权限恶意整套伪造。

作者 debug、开发测试和 E2E 通过后，从同一候选官方安装 CLI，验证版本及真实入口，然后取得一个独立架构 review PASS。完成适用 CI、正常 commit、PR/main 集成、远端回执及自有资源清理。没有 daemon 变更时不重启 daemon。每个完成/失败节点立即落盘证据；缺适用证据不得报告完成。

直接执行直到交付收口，不再生成下一层提示词。常规交付已授权，不重复请求确认。
```
