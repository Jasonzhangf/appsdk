<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T03:58:51.779056+00:00","entry_kind":"rule","id":"appsdk-collab-no-journal-identity-map","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["使用 identity-map 作为唯一身份源","保留 board 与消息持久化"],"axis":"object","axis_id":"collab.identity-map","clause_id":"collab.no-journal","effects":["journal 代码与测试移除"],"evidence":["req-collab-identity v1-v5"],"forbidden_actions":["journal 读写、重放、压缩","从历史重放恢复身份"],"owner":"Collab identity-map owner","rule":"Collab 不做 journal；持久身份是 ~/.collab/identity-map.json；保留 board 与消息通知持久化。","scenario":"设计或修改身份持久化","scope":"AppSDK Collab 通信与身份模型","state_after":"身份只由 identity-map 承载","state_before":"存在 journal 引用","trigger":"新增或保留持久化代码"},"revision":1},"source_refs":[],"tags":["architecture","collab","identity-map","journal","message-notification"],"updated_at":"2026-11-13T23:21:51.755956+00:00"} -->

# Collab 无 journal，只保留 board 与最近消息通知

用户确认（2026-11-06，会话 `01a1ae11` 讨论通过）：

1. AppSDK/Collab 只有任务 board，最近的消息通知持久化，不做 journal。
2. journal 整个设计都不需要，但 board 与消息通知持久化必须保留，不能移除。
3. 持久身份就是 identity map（`~/.collab/identity-map.json`）；
   `collab context` 恢复是否足够按实际场景验证，不够再讨论。

此决策与旧的 append-only reducer journal 语义冲突，后续文档、maps 与代码
如仍引用 journal 读写/维护逻辑，需按本决策消融；board 与消息通知持久化
不在移除范围内。
<!-- project-memory:end -->
