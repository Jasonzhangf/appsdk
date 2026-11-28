<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T09:44:41.718347+00:00","entry_kind":"rule","id":"appsdk-symbol-exploration-rg","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["使用 rg 与源码阅读"],"axis":"behavior","axis_id":"process.code.search","clause_id":"proc.symbol-search","effects":["无"],"evidence":["memory appsdk-symbol-exploration-rg"],"forbidden_actions":["使用或推荐 nn-code","把手工补齐结果冒称工具输出"],"owner":"项目 AGENTS owner","rule":"符号探索使用 rg 与直接源码阅读，不用已停用的 nn-code。","scenario":"探索符号、调用链或依赖","scope":"AppSDK 项目治理","state_after":"查询完成","state_before":"需要查询","trigger":"需要代码查询"},"revision":1},"source_refs":[],"tags":["agents","appsdk","code-search","symbols"],"updated_at":"2026-11-13T23:22:10.233582+00:00"} -->

# 符号探索用 rg 与直接源码阅读，不用已停用的 nn-code

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论）：

1. `nn-code` 已停用，不得用于项目查询或推荐调用。
2. 探索 TypeScript/JavaScript 符号、调用链和代码依赖时，使用 `rg` 与
   直接源码阅读。
3. 项目 `AGENTS.md` 的「符号探索」段如仍指向 `nn-code`，应改为
   `rg` 与直接源码阅读。

不得把手工补齐的结果冒称为工具输出。
<!-- project-memory:end -->
