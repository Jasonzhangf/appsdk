<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-11T05:09:43.978087+00:00","entry_kind":"fact","id":"appsdk-rex-delivery-status","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":null,"source_refs":[],"tags":["appsdk","collab","delivery","identity","rex","runtime"],"updated_at":"2026-11-13T23:22:09.276756+00:00"} -->

# Rex 已交付、测试中；daemon 有 identity 问题，判定初步可用

用户裁决（2026-11-11）：

1. Rex transport 已交付（源码合入 main）。
2. 当前仍在测试，未进入稳定生产状态。
3. appsdk 当前 daemon 不稳定，存在 identity 问题（例如 Henry/peer-8 同一 Rex block 的 duplicate active anchor，collab context 报 IDENTITY_MAP_AMBIGUOUS / IDENTITY_ANCHOR_CONFLICT）。
4. 结论：Rex 判定为初步可用，仍有 bug；测试候选验证与生产验收继续分开记录，不把交付冒充稳定。

Ontology：
- 场景：选择 Collab 宿主传输，或声明 Rex 能力。
- 触发：Rex 相关派单、收件或身份解析。
- 允许动作：可用 Rex 做测试与初步任务；身份冲突保留原错并交 identity owner 修复，不重放、不改控制状态。
- 影响状态：Rex transport=delivered+testing；daemon=unstable+identity bugs；生产可用=未达。
<!-- project-memory:end -->
