<!-- project-memory:v1 {"category":"path","created_at":"2026-11-11T13:49:01.805108+00:00","entry_kind":"rule","id":"appsdk-ontology-governance-flow","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["按固定顺序推进"],"axis":"behavior","axis_id":"governance.ontology.flow","clause_id":"gov.flow","effects":["无"],"evidence":["docs/design/appsdk-ontology-governance.md"],"forbidden_actions":["未审批条款驱动实现、debug 或验收","非规则事实进入审批"],"owner":"appsdk-project-governance Skill owner","rule":"受影响任务按三轴建模、用户逐条审批、DAG、gate 红测、唯一 owner 改码、真实入口验收的顺序推进。","scenario":"任务、debug 或治理升级影响对象/行为/关系","scope":"AppSDK 项目治理","state_after":"条款已审批、绑定 gate 并验收","state_before":"条款未建模","trigger":"受影响任务开始"},"revision":1},"source_refs":[],"tags":["appsdk","flow","gate","governance","ontology"],"updated_at":"2026-11-13T23:22:03.668501+00:00"} -->

# AppSDK 治理流程采用三轴 ontology 并绑定 gate

用户 2026-11-11 指令：三轴 ontology 用于本项目治理，加入 AppSDK 治理流程。已写入项目 AGENTS.md 新章节「Ontology 治理流程」。

固定顺序（受影响任务）：
1. 先整理三轴 ontology（Object/Behavior/Relation），只覆盖当前变更或排障必需。
2. 进入审批范围的规则由用户逐条确认；未确认前是候选。
3. 按确认 ontology 生成或更新治理 DAG。
4. 更新受影响 gate 并取得红测证据。
5. 修改唯一 owner 代码，再从真实入口验收。
中途新增/修改沿用相同顺序，并继续消费已确认项目记忆。

强制要求：每条 ontology 规则绑定可执行治理 edge/gate（identity/attribute/precondition/transition/relation/terminal/effect 之一）并给出合法与违规样例；无 gate 的条款是候选，不得当作强制规则或用于实现/debug/验收结论。requirements.json 只是声明、无强制力（appsdk-governance-edge-gate-requirement）。非规则事实不进审批。

Ontology：
- 场景：AppSDK 本项目治理，需要建模业务语义或落实治理。
- 触发：任务变更、debug 或治理升级影响对象/行为/关系。
- 允许动作：按三轴建模 -> 用户审批 -> 生成 DAG -> 更新 gate -> 改代码 -> 真实入口验收。
- 影响状态：条款从 candidate 升级为 approved+governed；未绑定 gate 或未审批的仍是候选。
<!-- project-memory:end -->
