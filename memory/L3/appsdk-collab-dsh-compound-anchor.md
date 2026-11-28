<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T09:58:19.172505+00:00","entry_kind":"rule","id":"appsdk-collab-dsh-compound-anchor","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["sessionID 匹配现有名字时恢复"],"axis":"object","axis_id":"collab.dsh.anchor","clause_id":"collab.dsh-anchor","effects":["无"],"evidence":[".appsdk/requirements.json req-collab-identity v6","collab/src/server/identity_context/required_fields_tests.rs dsh_anchor_alone_requires_no_appserver_fields"],"forbidden_actions":["要求额外 communication API 锚点才恢复","sessionID 不匹配时凭其他历史恢复"],"owner":"Collab DSH binding owner","rule":"DSH 与 Codex Desktop 只凭 sessionID 即可恢复身份。","scenario":"DSH 身份恢复","scope":"AppSDK Collab 通信与身份模型","state_after":"DSH/Desktop 身份按 sessionID 恢复","state_before":"DSH 身份待恢复","trigger":"DSH context 或 daemon 恢复"},"revision":2},"source_refs":[],"tags":["anchor","collab","dsh","identity","sessionID","test"],"updated_at":"2026-11-13T23:21:48.449808+00:00"} -->

# DSH 与 Codex Desktop 只凭 sessionID 即可恢复身份

用户裁决（2026-11-13，Claude Code 会话 ee987f9f，原话：“dsh codext desktop 只凭 sessionID可以恢复”）：

1. DSH 与 Codex Desktop 只凭 sessionID 即可恢复身份，不要求额外的 communication API 锚点。
2. 本裁决替代 2026-11-07 的“DSH 须用 sessionID + communication API 复合锚点”旧结论。
3. 需求账本已记录为 req-collab-identity v6。
<!-- project-memory:end -->
