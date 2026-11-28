<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T11:51:10.802674+00:00","entry_kind":"rule","id":"appsdk-layerfs-fully-cleanup","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["移除 LayerFS 内容","统一用 Git worktree"],"axis":"behavior","axis_id":"process.layerfs.remove","clause_id":"proc.layerfs","effects":["Git worktree 为唯一载体"],"evidence":["memory appsdk-layerfs-fully-cleanup"],"forbidden_actions":["保留兼容或迁移方案","未核实未交付内容就删除存量"],"owner":"对应载体 owner","rule":"LayerFS 已退役；遇到 LayerFS 引用、载体或兼容路径就移除；新任务只用 Git worktree。","scenario":"创建任务载体或发现 LayerFS 引用","scope":"AppSDK 项目治理","state_after":"LayerFS 引用为 0","state_before":"存在 LayerFS 引用","trigger":"遇到 LayerFS 相关内容"},"revision":1},"source_refs":[],"tags":["appsdk","architecture","cleanup","layerfs","worktree"],"updated_at":"2026-11-13T23:22:00.355727+00:00"} -->

# LayerFS 完全清理，新工作使用 Git worktree

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论；2026-11-05 全局停用
LayerFS；2026-11-11 用户追加裁决：遇到就移除）：

1. LayerFS 不做保留性兼容，所有 LayerFS 相关工作与存量载体按用户指示
   完全清理。
2. 新任务只使用 Git worktree，且路径与分支名带语义日期后缀。
3. 已有 LayerFS 未交付内容由原 owner 先核实并收口，再按本规则清理；
   清理动作由对应 owner 执行，不等于本记忆条目本身授权立即删除。
4. 项目内仍引用 LayerFS 双支持的兼容目标、schema 或代码路径，后续
   统一改为移除或只读历史，不再作为新工作前置。
5. LayerFS 已退役；遇到 LayerFS 相关引用、载体或兼容路径就移除，
   不保留第二套交付载体，也不再新增迁移或兼容方案。

Ontology（2026-11-11 翻新）：

- 场景：创建/迁移任务载体，或发现 LayerFS 引用与兼容路径。
- 触发：遇到 LayerFS 相关载体、schema、代码路径或旧引用。
- 允许动作：移除 LayerFS 相关内容，统一改用 Git worktree；存量删除仍由
  对应 owner 核实未交付内容后执行。
- 影响状态：项目内 LayerFS 引用数降为 0；Git worktree 成为唯一执行载体。
<!-- project-memory:end -->
