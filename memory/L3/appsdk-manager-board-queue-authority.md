<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T12:21:19.595868+00:00","entry_kind":"rule","id":"appsdk-manager-board-queue-authority","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["修改 scope 内 peer 任务"],"axis":"relation","axis_id":"collab.authority.manager-scope","clause_id":"collab.manager-queue","effects":["无"],"evidence":["memory appsdk-manager-board-queue-authority"],"forbidden_actions":["修改巡查员任务","把该授权扩到消息发送权限"],"owner":"Collab typed authority owner","rule":"manager 可修改其 scope 内 peer 的任务 board 和 queue，不得修改巡查员任务。","scenario":"manager 调整任务","scope":"AppSDK Collab board","state_after":"任务按 manager 修改更新","state_before":"任务归 scope 内 peer","trigger":"board/queue 修改请求"},"revision":1},"source_refs":[],"tags":["authority","board","collab","inspector","manager","peer","queue"],"updated_at":"2026-11-13T23:22:00.835773+00:00"} -->

# Manager 可改 scope 内 peer 的任务 board 与 queue，不改巡查员任务

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论）：

1. 新设计中 manager 可以修改其管理 scope 内 peer 的任务 board 和 queue。
2. manager 不得修改巡查员的任务。
3. 该边界属于 manager/peer/巡查员任务授权，不改变消息发送权限。
<!-- project-memory:end -->
