<!-- project-memory:v1 {"category":"path","created_at":"2026-11-22T03:34:25.312069+00:00","entry_kind":null,"id":"collab-task-execution-contract-dag","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":null,"source_refs":[],"tags":["collab","dag","ontology","workflow"],"updated_at":"2026-11-22T03:34:25.312069+00:00"} -->

# Collab 任务执行合同 DAG

标准语义源见 collab/skills/collab/references/task-execution-contract.graph.json。一次任务事件从当前身份、任务与证据进入，选择原宿主注册、派发、执行、原位取证、阻塞恢复或验收合同，汇合到本轮可核对结果。恢复和返工由新事件重新进入。Jenny 调度与回收，architect 详细设计、直接交接、解阻和验收，指定 peer 执行、直接报阻塞并交付。可复制命令及参数来源见 collab/skills/collab/operations.md#task-contract-state。运行态以 collab context、collab board show 与实际执行回执为准。
<!-- project-memory:end -->
