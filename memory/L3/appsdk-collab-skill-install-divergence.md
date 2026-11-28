<!-- project-memory:v1 {"category":"knowledge","created_at":"2026-11-11T05:36:09.600674+00:00","entry_kind":"fact","id":"appsdk-collab-skill-install-divergence","importance":0,"memory_level":3,"review_evidence":[],"review_status":"unreviewed","rule_control":null,"source_refs":[],"tags":["appsdk","collab","divergence","fact","install","skill"],"updated_at":"2026-11-13T23:21:55.092270+00:00"} -->

# 已装 collab Skill 与仓库 origin/main 存在差异

非规则项目事实，直接记录不进审批（依据 appsdk-memory-approval-scope）。

事实（2026-11-13 本地 diff -rq 复核，仓库 origin/main 56548a85）：
1. 已装 ~/.agents/skills/collab 与仓库 collab/skills/collab 仍有差异。
2. 内容不同的文件：SKILL.md、operations.md、references/resource-waits.md、references/stable-name-recovery.md、references/task-worktree-lifecycle.md。
3. 两侧文件集合已一致，没有单侧独有文件。
处理规则：重新同步或全局安装按 appsdk-dev-installer-approval-gate（runtime 全局安装逐次批准），不自动执行。
Owner：用户 + skill owner。
<!-- project-memory:end -->
