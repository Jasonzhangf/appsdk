<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-07T10:17:30.476222+00:00","entry_kind":"rule","id":"appsdk-worktree-name-date-suffix","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":{"approval":{"state":"candidate"},"clause":{"allowed_actions":["使用带日期后缀的名字"],"axis":"object","axis_id":"process.worktree.name","clause_id":"proc.worktree-name","effects":["无"],"evidence":[".agents/skills/appsdk-dev/SKILL.md"],"forbidden_actions":["只用 slug 或短 hash","为改名中断已在运行的任务"],"owner":"appsdk::delivery","rule":"新任务 worktree 与分支名使用 <task-slug>-<yyyymmdd>。","scenario":"创建新任务 worktree 或分支","scope":"AppSDK 项目治理","state_after":"worktree 与分支名带日期","state_before":"无 worktree","trigger":"新任务开始"},"revision":1},"source_refs":[],"tags":["appsdk","branch","delivery","naming","worktree"],"updated_at":"2026-11-13T23:22:12.063722+00:00"} -->

# AppSDK 新任务工作树与分支名须带日期后缀

用户确认（2026-11-07，AppSDK 记忆 agent 逐项讨论）：

1. `appsdk-dev` Skill 的旧模板用 `/Volumes/Intel/playground/appsdk/<task-slug>` 和分支 `codex/`，缺少日期，与全局命名规则冲突。
2. 后续新任务的 worktree 和分支都应使用 `<task-slug>-<yyyymmdd>`，例如路径 `/Volumes/Intel/playground/appsdk/<task-slug>-<yyyymmdd>`，分支 `codex/<task-slug>-<yyyymmdd>`。
3. 已在运行的任务不为改名中断；此规则只约束后续新任务。
4. `appsdk-dev/SKILL.md` 的模板需按此规则更新，owner 为 `appsdk::delivery`。
<!-- project-memory:end -->
