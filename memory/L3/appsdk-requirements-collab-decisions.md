<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T08:58:00.229521+00:00","entry_kind":"rule","id":"appsdk-requirements-collab-decisions","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["同步写入三个决策"],"axis":"relation","axis_id":"appsdk.requirements.collab","clause_id":"appsdk.ledger-collab-decisions","effects":["无"],"evidence":[".appsdk/requirements.json v13-v17"],"forbidden_actions":["回退到 pane 锚点或 master-only 普通消息"],"owner":"requirements ledger owner","rule":"需求账本须体现 identity-map 身份、无 journal、任意对象直发普通消息三个决策。","scenario":"更新需求账本","scope":"AppSDK 需求账本","state_after":"账本包含三个决策","state_before":"账本缺少决策","trigger":"requirements 变更"},"revision":1},"source_refs":[],"tags":["appsdk","collab","identity-map","journal","message-send","requirements"],"updated_at":"2026-11-13T23:22:07.124750+00:00"} -->

# 需求账本需体现三个 Collab 决策

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论）：

1. 持久身份以 `~/.collab/identity-map.json` 为准；不看 journal 或旧历史。
2. Collab 不做 journal；保留任务 board 与最近消息通知持久化。
3. 任意两个已注册对象可直接发送普通消息；任务派发与项目管理授权保留。

`.appsdk/requirements.json` 当前版本仍保留 pane 身份锚点与旧绑定措辞。
后续更新需求账本时，必须同步体现以上三个决策；不得回退到 pane 锚点或
master-only 普通消息限制。
<!-- project-memory:end -->
