<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T06:15:12.840791+00:00","entry_kind":"rule","id":"appsdk-collab-direct-message-send","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["同项目和跨项目任意组合发送","保留真实发送者认证"],"axis":"behavior","axis_id":"collab.message.send","clause_id":"collab.direct-send","effects":["无管理权限变化"],"evidence":["req-collab-communication-notifications v6"],"forbidden_actions":["因发送者不是 master 拒绝普通消息","把发送当作任务分派或管理授权","冒充其他发送者"],"owner":"Collab mailbox/send owner","rule":"任意两个已注册对象可直接发送普通消息，不限 master；任务派发与项目管理授权仍保留。","scenario":"已注册对象发送普通消息","scope":"AppSDK Collab 通信与身份模型","state_after":"消息入 daemon 持久化并派发","state_before":"消息待发送","trigger":"send 或 CrossProjectSend"},"revision":1},"source_refs":[],"tags":["authorization","collab","message","send"],"updated_at":"2026-11-13T23:21:47.050759+00:00"} -->

# 任意两个已注册对象可直接发送普通消息

用户确认（2026-11-06，AppSDK 记忆 agent 逐项讨论；原话见会话 `01a1ae11`）：

1. 任意两个已注册对象可以直接发送普通消息，不再限 master。
2. 任务派发与项目管理授权仍然保留，直发消息不等于获得管理权限。
3. `CrossProjectSend` 不应因发送者不是 master 而拒绝普通消息。

跨项目发送的身份限制取消；权限收敛只针对任务派发和控制操作。
<!-- project-memory:end -->
