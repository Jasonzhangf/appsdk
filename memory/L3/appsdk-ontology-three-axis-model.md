<!-- project-memory:v1 {"category":"path","created_at":"2026-11-11T06:07:33.678849+00:00","entry_kind":"rule","id":"appsdk-ontology-three-axis-model","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["按三轴填写模板","每条规则绑定一种 gate kind"],"axis":"object","axis_id":"governance.ontology.template","clause_id":"gov.three-axis","effects":["无"],"evidence":["sdk-skill-sources/appsdk-project-governance/references/ontology-governance.md"],"forbidden_actions":["无 gate 的条款冒充强制规则"],"owner":"appsdk-project-governance Skill owner","rule":"ontology 按 Object/Behavior/Relation 三轴建模；每条条款绑定可执行 gate 才从 candidate 升为生效。","scenario":"整理 AppSDK ontology 或治理规范","scope":"AppSDK 项目治理","state_after":"条款为 approved+governed 或仍为 candidate","state_before":"条款为 candidate","trigger":"建模对象、行为、关系"},"revision":1},"source_refs":[],"tags":["appsdk","behavior","constraint","governance","object","ontology","relation","template"],"updated_at":"2026-11-13T23:22:04.457201+00:00"} -->

# Ontology 按 Object/Behavior/Relation 三轴建模并绑定治理 gate

用户 2026-11-11 指令：ontology 从三个方向建模，并作为我方建模模板与最终治理规范模板。

三轴：
1. Object（它是什么）：属性、被感知的方法、must_have/must_not_have 属性约束。
2. Behavior（它做什么）：在什么状态下、什么输入、什么输出、引起什么状态变化，可有允许/禁止转移约束。
3. Relation（它和别的对象有什么关系）：关系类型、方向、基数、被感知方法，及 must_have/must_not_have 关系约束。

治理要求：每条 ontology 条款必须绑定可执行 edge/gate 才能从 candidate 升级为生效治理；requirements.json 只是表述、没有强制力（appsdk-governance-edge-gate-requirement）。没有 gate 的条款仍是候选，不得冒充强制规则。

模板 v1：.appsdk-control/memory-agent-runs/20261111T0607Z-ontology-three-axis-template.md，含元模型、三轴字段、约束分类、ontology->edge/gate 映射、五维 DAG 对照和 JSON 骨架。

Ontology：
- 场景：整理或实现 AppSDK 业务 ontology 与治理规范。
- 触发：需要建模对象、行为、关系，或把 ontology 落成 gate。
- 允许动作：按三轴填写模板；每条规则绑定 identity/attribute/precondition/transition/relation/terminal/effect 之一；无 gate 标 candidate。
- 影响状态：条款从 candidate 升级为 approved+governed；未绑定 gate 的仍是候选。
<!-- project-memory:end -->
