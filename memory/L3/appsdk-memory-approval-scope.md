<!-- project-memory:v1 {"category":"path","created_at":"2026-11-11T05:31:52.077192+00:00","entry_kind":"rule","id":"appsdk-memory-approval-scope","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["规则提交审批","事实直接记录"],"axis":"behavior","axis_id":"memory.approval.classify","clause_id":"mem.approval-scope","effects":["无"],"evidence":["sdk-skill-sources/project-memory/SKILL.md"],"forbidden_actions":["事实进入审批队列","事实冒充规则"],"owner":"project-memory owner","rule":"只有规则与 ontology 条款进入审批；与规则无关的项目事实直接记录并保留证据。","scenario":"判断条目是否需要用户审批","scope":"AppSDK 项目记忆","state_after":"条目为 fact 或 rule candidate","state_before":"条目未分类","trigger":"整理出一条项目真相或规则"},"revision":1},"source_refs":[],"tags":["approval","appsdk","fact","memory","ontology"],"updated_at":"2026-11-13T23:22:02.339606+00:00"} -->

# 只有规则类条目进入审批，项目真相不进入审批

用户确认（2026-11-11）：

1. 和规则没关系的项目真相不要进入审批。
2. 审批队列只包含规则与 ontology 条款；与规则无关的项目事实按事实记录并保留证据。
3. 事实记录不因未经审批而丢失，但不得冒充规则或治理约束。

Ontology：
- 场景：记忆 agent 判断条目是否需要用户审批。
- 触发：整理出一条项目真相或规则。
- 允许动作：规则/ontology 条款提交审批；非规则事实直接记录并附证据。
- 影响状态：审批队列只含规则类条目；事实仍可检索。
<!-- project-memory:end -->
