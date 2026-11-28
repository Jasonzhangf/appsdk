<!-- project-memory:v1 {"category":"plan","created_at":"2026-11-07T11:52:18.226399+00:00","entry_kind":"rule","id":"appsdk-session-discovery-inspector-merge","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["交巡检员合并","用户批准后入库"],"axis":"behavior","axis_id":"memory.intake.session-discovery","clause_id":"mem.session-discovery","effects":["无"],"evidence":["memory appsdk-session-discovery-inspector-merge"],"forbidden_actions":["直接写入项目记忆"],"owner":"项目巡检员与 project-memory owner","rule":"会话发现沉淀先交项目巡检员合并，再由用户审批后入库记忆。","scenario":"整理会话发现","scope":"AppSDK 项目记忆","state_after":"发现经合并与批准入库","state_before":"发现未合并","trigger":"产生会话发现条目"},"revision":1},"source_refs":[],"tags":["approval","codex-dsh","inspector","memory-ingestion","merge","session-discovery"],"updated_at":"2026-11-13T23:22:09.525768+00:00"} -->

# 会话发现沉淀由项目巡检员合并，再由用户审批入库

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论）：

1. 会话发现与沉淀（包括 C4 会话源清单）不直接写入项目记忆；
   先提交给项目巡检员（Frank）合并整理。
2. 合并后由项目巡检员提交用户审批，审批通过后才入库记忆。
3. Memory agent 本轮只记录此路由规则，不替代巡检员合并与用户审批。
<!-- project-memory:end -->
