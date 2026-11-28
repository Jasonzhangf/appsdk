<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T04:08:31.881707+00:00","entry_kind":"rule","id":"appsdk-inbox-persist-notification-trigger","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["agent 查询历史消息","通知丢弃后仍能从收件箱读到"],"axis":"relation","axis_id":"collab.notification-mailbox","clause_id":"collab.inbox-persist","effects":["无"],"evidence":["memory appsdk-inbox-persist-notification-trigger"],"forbidden_actions":["因未读通知阻塞身份交接或恢复","把通知当消息真源"],"owner":"Collab mailbox/notification owner","rule":"收件箱持久保存消息；通知只是触发回读，通知丢失不构成消息丢失。","scenario":"消息投递与通知","scope":"AppSDK Collab 通信与身份模型","state_after":"消息保留，通知状态独立","state_before":"消息已入收件箱","trigger":"消息入库或通知发出"},"revision":1},"source_refs":[],"tags":["collab","inbox","mailbox","message-retention","notification"],"updated_at":"2026-11-13T23:21:57.933447+00:00"} -->

# 通知只是触发，收件箱持久化保留历史消息

用户确认（2026-11-06，AppSDK 记忆 agent 逐项讨论）：

1. 收件箱有持久化保存机制，循环保留持久化内容。
2. 通知只是通知，用来触发 agent 回读；agent 可以查询历史消息。
3. 通知没读没关系，下次仍能看到，读不读、丢不丢由 agent 自己决定。
4. 通知丢弃不会丢信息，丢了就算了。

信息以收件箱持久化内容为准，通知丢失不构成消息丢失，不因未读通知阻塞
身份交接或恢复。
<!-- project-memory:end -->
