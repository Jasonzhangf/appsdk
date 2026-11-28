<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-11T05:35:36.448960+00:00","entry_kind":"fact","id":"appsdk-worktree-limit-current-state","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":null,"source_refs":[],"tags":["appsdk","fact","governance","limit","worktree"],"updated_at":"2026-11-13T23:22:10.867681+00:00"} -->

# Appsdk worktree 上限现状：20 个、达到上限

非规则项目事实，直接记录不进审批（依据 appsdk-memory-approval-scope）。

事实（2026-11-13 本地复核）：
1. 全局 AGENTS 当前声明每项目最多 20 个 worktree（含主工作树）；自 2026-10-30 04:36 UTC 起超过 20 个禁止提交。旧记录中的 15 已过时。
2. 本次 git worktree list 复核 appsdk 共 20 个 worktree（含主树 /Users/fanzhang/Documents/github/appsdk），恰好达到上限。
3. appsdk-dev Skill 声明了超过 20 个时的提交门禁及其正反用例（.agents/skills/appsdk-dev/SKILL.md）。
含义：计数随时间漂移，引用前应重新运行 git worktree list。
<!-- project-memory:end -->
