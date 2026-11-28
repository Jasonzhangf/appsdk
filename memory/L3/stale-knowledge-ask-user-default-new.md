<!-- project-memory:v1 {"category":"lesson","created_at":"2026-11-07T11:52:18.226399+00:00","entry_kind":"rule","id":"stale-knowledge-ask-user-default-new","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["先询问用户","无回复按新规则覆盖"],"axis":"behavior","axis_id":"memory.stale.resolve","clause_id":"mem.stale-ask","effects":["无"],"evidence":["memory stale-knowledge-ask-user-default-new"],"forbidden_actions":["不问用户直接覆盖冲突项","因可能冲突长期等待"],"owner":"project-memory owner","rule":"覆盖陈旧知识或处理冲突前先问用户；无回复时默认按新的规则覆盖。","scenario":"发现陈旧或冲突知识","scope":"AppSDK 项目记忆","state_after":"条目按确认或默认新规则更新","state_before":"存在陈旧条目","trigger":"准备传播或覆盖"},"revision":1},"source_refs":[],"tags":["conflict","default-new","memory-update","stale-knowledge","user-approval"],"updated_at":"2026-11-13T23:22:12.822299+00:00"} -->

# 陈旧知识要询问用户，冲突尤其询问，无回复默认按新的来

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论）：

1. 传播或覆盖陈旧知识、项目要求冲突前，必须先询问用户，尤其冲突项。
2. 用户无回复时，默认按新的用户确认/规则覆盖旧知识，不阻塞推进。
3. 不因“可能有冲突”长期等待；确认后按当前有效规则更新记忆。
<!-- project-memory:end -->
