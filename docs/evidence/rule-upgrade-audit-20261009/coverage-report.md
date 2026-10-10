交付完成。A（coverage）只读补查的两份产物已落盘：

- [notes.md](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/coverage/notes.md)
- [report.md](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/coverage/report.md)

**做了什么**

对 core 覆盖表偏薄弱的活跃链逐类补查入口/owner 真源/控制真源/失败调用边/consumer·test 边界：

- compile：`main.rs:1225-1255`、`project.rs:443-471`、`compile.rs:1074-1200,1324-1377`
- promotion：`main.rs:1288-1318`、`promotion.rs:3-544,864-1319`
- verification：`cli.rs:3-37`、`promotion.rs:864-1319`、`verification.rs:359-874`
- lifecycle/producer：`main.rs:1320-1381`、`lifecycle_chain.rs:3-552`、`producer.rs:395-432`、`producer_commit.rs:98-412`
- merge：`lifecycle_closure.rs:837-977`、`merge_gates.rs:3-403`
- reset：`main.rs:1218-1224,1415-1428`、`reset_governance.rs:315-318`、`bug_tracker.rs:3-47`、`reset_run.rs:3-509`

结论：六类各有唯一 CLI 入口、唯一控制真源、typed 失败码和对应 smoke 测试边界，**覆盖表足以支撑 controller 组合 release 批次**，无需本 worker 补链或框架。

**关键 caveat**：HEAD 仍是基线 `3dfdaf85`，但工作树已含他人在途的 `0.1.0013` 迁移改动（`init.rs` 写前 pin 拒绝、`reset_governance.rs` 新步骤、`governance.rs` 的 `0.1.0012` 历史 map/manifest、测试字面量更新等）。我读的是当前工作树、以当前行号为准，**不判断这些在途 hunk 的正确性**，也不把工作树表述为基线。

**明确 unknown（已标注，未冒充全读）**：`compile.rs:481-1073` 内部语义、`promotion.rs:324-544` active publish 全失败分支、`verification.rs:36-216` schema assert、`merge_gates.rs:447-860` 全 graph、`review_context/review_gates` 分支语义；`reset-staging-scaffold` 无直接 CLI 测试（UNVERIFIED）。

**未改产品源码，无 patch**；未 build/test/install，未 commit/merge/push。工作树 `git status` 与进入时一致（33 项，均为他人改动）。