<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T12:21:19.595868+00:00","entry_kind":"rule","id":"appsdk-dev-installer-approval-gate","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["引用全局规则"],"axis":"behavior","axis_id":"delivery.install.approval","clause_id":"proc.installer-approval","effects":["无"],"evidence":[".agents/skills/appsdk-dev/SKILL.md"],"forbidden_actions":["新建第二道门禁","省略引用"],"owner":"appsdk::delivery","rule":"appsdk-dev Skill 的安装流程必须引用全局 runtime 全局安装逐次批准规则，不另建门禁。","scenario":"修订 appsdk-dev 安装流程","scope":"AppSDK 项目治理","state_after":"Skill 引用全局规则","state_before":"Skill 缺引用","trigger":"修改安装说明"},"revision":1},"source_refs":[],"tags":["approval","appsdk","delivery","installer","skill"],"updated_at":"2026-11-13T23:21:57.038283+00:00"} -->

# appsdk-dev 安装流程须引用全局逐次批准门禁

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论）：

1. `appsdk-dev` Skill 中安装器/安装流程必须引用全局
   “runtime 全局安装逐次批准”门禁。
2. 不新建第二道门禁；项目 Skill 只引用全局规则。
3. QQBot 接收器已有全局例外，按全局规则执行。
4. Owner 为 `appsdk::delivery`；后续修订 Skill 时补齐引用。
<!-- project-memory:end -->
