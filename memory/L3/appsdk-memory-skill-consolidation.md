<!-- project-memory:v1 {"category":"plan","created_at":"2026-11-07T11:52:18.226399+00:00","entry_kind":"rule","id":"appsdk-memory-skill-consolidation","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["合并为一个 Skill"],"axis":"object","axis_id":"memory.skill.single","clause_id":"mem.skill-merge","effects":["无"],"evidence":["memory appsdk-memory-skill-consolidation"],"forbidden_actions":["给各记忆 Skill 再加交叉引用","另建重复技能条目"],"owner":"合并 Skill owner","rule":"conversation-memory、project-memory、memmy-memory 等记忆技能合并为一个 Skill。","scenario":"维护记忆相关 Skill","scope":"AppSDK 项目记忆","state_after":"只有一个记忆 Skill","state_before":"存在多个记忆 Skill","trigger":"修改记忆 Skill 或交叉引用"},"revision":1},"source_refs":[],"tags":["consolidation","conversation-memory","memmy-memory","memory","project-memory","skill"],"updated_at":"2026-11-13T23:22:03.179621+00:00"} -->

# Memory 技能合并为一个 Skill

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论）：

1. 不再给 conversation-memory 分别加 project-memory 交叉引用；
   Memory 相关技能（conversation-memory、project-memory、memmy-memory 等）
   应合并为一个 Skill。
2. 合并后由合并 Skill 的 owner 负责路径、交叉引用与六小时重装/周期等流程；
   本记忆只记录合并决策，不代表合并已完成。
3. 合并实施应由项目巡检员先合并会话发现沉淀，再经用户审批入库；
   不允许 memory agent 直接另建重复技能条目。
<!-- project-memory:end -->
