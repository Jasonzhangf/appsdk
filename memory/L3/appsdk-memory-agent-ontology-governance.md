<!-- project-memory:v1 {"category":"path","created_at":"2026-11-11T05:09:36.221603+00:00","entry_kind":"rule","id":"appsdk-memory-agent-ontology-governance","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["提交逐条候选","批准后经正式入口写入"],"axis":"behavior","axis_id":"memory.rule.promote","clause_id":"mem.agent-approval","effects":["待批时唤醒暂停"],"evidence":["contracts/memory/rule-promotion.ontology.json"],"forbidden_actions":["以 review PASS 或任务验收代替记忆批准","有待批候选时续订唤醒","未批改语义"],"owner":"project-memory owner","rule":"记忆晋升的唯一审批人是用户；存在待批候选时不订阅下一次唤醒；旧记忆翻新须经用户审批，未批不改语义。","scenario":"记忆 agent 整理候选或翻新旧记忆","scope":"AppSDK 项目记忆","state_after":"条目经用户批准晋升","state_before":"条目为候选","trigger":"候选整理完成"},"revision":1},"source_refs":[],"tags":["approval","appsdk","governance","memory","ontology"],"updated_at":"2026-11-13T23:22:01.539327+00:00"} -->

# 记忆 agent 的 ontology 治理与审批门禁

用户确认（2026-11-11，AppSDK ontology 试点逐条审批）：

1. 记忆晋升的唯一审批人是用户；独立 review PASS 与 master 任务验收都不是记忆批准。
2. 独立 review、任务验收、记忆晋升是三个 owner，互不传递。
3. 存在待批候选时不订阅、不续订下一次唤醒；候选清零后才重新 arm 同一目标。
4. 每条 durable 记忆按 ontology 记录：生效场景、触发事件、允许动作、影响状态；无法定位场景与状态的条目不入库。
5. 共享记忆真源是 ~/.agents/MEMORY.md；~/.codex/memories/MEMORY.md 只是 Codex 检索索引。
6. 旧记忆缺少上述字段的须翻新；翻新同样经用户审批后入库，未批不改语义。

Ontology：
- 场景：记忆 agent 从用户输入、代码和事故整理记忆或规则候选。
- 触发：候选整理完成，或既有记忆缺少 ontology 字段。
- 允许动作：提交逐条候选等待用户批准；批准后经正式入口写入 durable memory。
- 影响状态：候选批准前保持非 durable；批准后成为可被实现/debug 消费的确认语义。
<!-- project-memory:end -->
