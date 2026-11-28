<!-- project-memory:v1 {"category":"plan","created_at":"2026-11-11T05:31:44.062808+00:00","entry_kind":"rule","id":"appsdk-governance-edge-gate-requirement","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["把约束实现为 edge/gate 校验","requirements.json 记录表述与来源"],"axis":"behavior","axis_id":"governance.constraint.enforce","clause_id":"gov.edge-gate","effects":["约束具备强制力"],"evidence":["docs/design/appsdk-ontology-governance.md"],"forbidden_actions":["以文档声明冒充治理执行","把 requirements 条目当 gate"],"owner":"AppSDK 治理与 DAGPipe owner","rule":"requirements.json 只是表述；只有落到可执行 edge/gate 的治理约束才有强制力。","scenario":"设计或修改配置化治理","scope":"AppSDK 项目治理","state_after":"约束绑定 edge/gate，未绑定的仍是缺口","state_before":"约束只有表述","trigger":"新增或变更治理约束"},"revision":1},"source_refs":[],"tags":["appsdk","dagpipe","gate","governance","requirements"],"updated_at":"2026-11-13T23:21:57.494663+00:00"} -->

# 配置化治理须落到 edge 和 gate，requirements.json 只是表述

用户确认（2026-11-11，AppSDK 记忆 agent ontology 审批）：

1. requirements.json 没有强制力，只是一个表述。
2. 配置化治理架构需要更好的方式，把治理约束落实到可执行的治理 edge 和 gate。
3. 只有落到 edge/gate 的约束才算有强制力；文档声明不能冒充治理执行。

Ontology：
- 场景：设计或修改配置化治理架构。
- 触发：需要新增或变更治理约束。
- 允许动作：把约束实现为治理 edge 与 gate 校验；requirements.json 记录表述与来源。
- 影响状态：约束具备可执行强制力；未落到 edge/gate 的仍是待实现缺口。
<!-- project-memory:end -->
