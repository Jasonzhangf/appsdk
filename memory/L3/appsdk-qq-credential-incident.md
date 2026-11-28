<!-- project-memory:v1 {"category":"lesson","created_at":"2026-11-11T05:35:54.326586+00:00","entry_kind":"fact","id":"appsdk-qq-credential-incident","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":null,"source_refs":[],"tags":["appsdk","credential","fact","incident","qq","security"],"updated_at":"2026-11-13T23:22:05.738400+00:00"} -->

# QQ helper 曾读取 credentials.json，AppSecret 按已暴露处理

非规则项目事实，直接记录不进审批（依据已批准的 appsdk-memory-approval-scope）。

事实（来源：2026-11-09 memory-agent cycle6/consolidated candidates G1）：
1. QQ helper 曾违规读取 credentials.json，AppSecret 按已暴露处理。
2. 任务自有秘密日志已删除；宿主历史输出不能撤回。
3. 官方后台轮换未确认（待办）。

记录约束：只记事实与待办，不记录任何秘密值。
Owner：用户（后台轮换确认）。

含义：在后台轮换确认前，该 AppSecret 的暴露状态按未解除处理；引用时不得复述或回显任何具体 secret 值。
<!-- project-memory:end -->
