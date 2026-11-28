<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T10:26:04.358772+00:00","entry_kind":"rule","id":"appsdk-memory-patrol-6h-inspector-1h","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["各自保持自己的周期"],"axis":"object","axis_id":"process.patrol.cadence","clause_id":"proc.patrol-cadence","effects":["无"],"evidence":["memory appsdk-memory-patrol-6h-inspector-1h"],"forbidden_actions":["用一个周期覆盖另一个","巡查员占用 6 小时周期"],"owner":"记忆 agent 与代码巡查员","rule":"记忆 agent 巡检 6 小时一次，代码巡查员 1 小时一次，两者互不覆盖。","scenario":"设置或核对巡检周期","scope":"AppSDK 项目治理","state_after":"两个订阅各自生效","state_before":"两个订阅存在","trigger":"巡检订阅变更"},"revision":1},"source_refs":[],"tags":["appsdk","cadence","inspector","memory","patrol"],"updated_at":"2026-11-13T23:22:02.796802+00:00"} -->

# 记忆巡检 6h 专属：记忆 agent 专用；代码巡查员 1h

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论；此前纠正过把
4 小时巡查记录当作记忆巡检冲突的判断）：

1. 记忆 agent 巡检为 6 小时一次，使用订阅
   `sub-m1793977443105-124`，`interval_ms=21600000`。
2. 代码巡查员为 1 小时一次。其 authority 已从旧 4 小时收敛为 1 小时，
   使用 `project-code-patrol-1h` 订阅 `sub-m1794032904316-1108`，
   `interval_ms=3600000`。
3. 两个任务的角色和目标不同，不能把周期差异视为同一任务的冲突，
   也不能用一个周期覆盖另一个。
4. 旧的 4 小时 `project-code-patrol` 订阅是巡查员的历史记录；
   记忆巡检仍按 6 小时执行。
5. 六小时巡检是记忆 agent 专属；代码巡查员不占用、不变更该周期。
   巡查员只保留其自己的 1 小时代码巡检订阅。
<!-- project-memory:end -->
