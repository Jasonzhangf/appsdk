<!-- project-memory:v1 {"category":"path","created_at":"2026-11-11T05:31:21.985658+00:00","entry_kind":"rule","id":"appsdk-implementation-execution-mode","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["默认编排流程","按要求串行独立实现"],"axis":"behavior","axis_id":"process.implementation.mode","clause_id":"proc.exec-mode","effects":["责任留在受托执行者"],"evidence":["memory appsdk-implementation-execution-mode"],"forbidden_actions":["把实现误读为只出 plan"],"owner":"受托执行者","rule":"用户指派实现时默认按编排流程独立完成；用户明确要求时改为串行独立实现。","scenario":"用户指派实现任务","scope":"AppSDK 项目治理","state_after":"任务按所选方式执行","state_before":"任务待执行","trigger":"收到实现任务"},"revision":1},"source_refs":[],"tags":["appsdk","implementation","orchestration","user-preference"],"updated_at":"2026-11-13T23:21:57.693826+00:00"} -->

# 用户指派实现时按编排流程或串行独立实现

用户确认（2026-11-11，AppSDK 记忆 agent ontology 审批）：

1. 用户指派实现任务时，执行者独立按照编排流程完成（拆解、派发、review、交付）。
2. 如果用户明确要求不按照编排流程，则串行独立实现，不并发派发。
3. 不把「用户要实现」误读为「先产出独立 plan 再转做其他事」。

Ontology：
- 场景：用户指派实现任务。
- 触发：收到实现任务。
- 允许动作：默认按编排流程独立执行；用户明确要求时改串行独立实现。
- 影响状态：执行方式=编排流程或串行独立；责任留在受托执行者。
<!-- project-memory:end -->
