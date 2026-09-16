# AppSDK + Collab Initialization Prompts

Copy/paste these prompts into Codex TUI/Desktop. Each prompt declares the
project scope, owner, evidence, and the exact success signal so the
registration step never has to re-explore the project.

## Master init (after user approval for the exact project + peer)

```text
/goal
目标：在当前项目根完成 AppSDK + Collab 的全新 master 初始化，确保治理真源写入 ~/.appsdk 与 ~/.collab，并在 ~/.collab/routes.jsonl 与 <project>/.agent-collab/server/journal.jsonl 中找到完整注册证据。

实现文档：
- skills/appsdk-project-governance/SKILL.md
- skills/appsdk-project-governance/references/bootstrap-migration.md
- skills/appsdk-project-governance/references/state-paths.md
- skills/appsdk-project-governance/references/command-surface.md
- ~/.agents/skills/collab/SKILL.md
- ~/.agents/skills/collab/references/state-paths.md

执行规范：
- 前序控制面已清理：删除/保留 .appsdk、.appsdk-control、.agent-collab；只保留全局 ~/.appsdk、~/.collab 真源。如果旧控制面还在，停下先完成 fresh-start 步骤，不要带着残留注册。
- cd 到项目根，先跑 appsdk prepare，确认 .appsdk-prepare.json 的 status="confirmed"、change_kind="new_project"、confirmed_by/confirmed_at 已填写；未确认先停下询问用户，不要代替用户确认。
- 跑 appsdk init .；按真实 contract 改写 .appsdk/project.json / .appsdk/goal.json（占位符 change-me / goal-change-me / app-core 必须替换为真实 ID 与模块）。
- 跑 appsdk guide compile、appsdk verify；rg -n "change-me|goal-change-me|app-core" .appsdk 必须无命中。
- 必须在主仓库根目录运行 `appsdk init .` / `collab init`，禁止在 worktree 中注册。
- 在注册 Collab 之前先记录项目绝对路径、当前 pane/thread id；调用 collab init 一次（仅一次）。记录返回的 worker_id、runtime_id、binding_id、native_thread_id、token、app_scope_id。
- 按 ~/.agents/skills/collab/references/state-paths.md 中的 "Registration verification checklist" 逐项核对：~/.collab/routes.jsonl 出现 canonical_root=<abs project root> 行；<project>/.agent-collab/server/journal.jsonl 顺序出现 GlobalProjectRegistered、GlobalRuntimeBound、Registered、NotificationSubscribed、CommandCompleted，且所有事件里的 worker_id 完全一致。缺任一事件立即停下，不要伪造已注册。
- 用户已显式批准当前 peer 作为该项目 master 后再跑 collab master promote --approval "<用户原文>"；未批准前不要 promote，也不要把 appsdk init 完成等同于 master 权限。
- collab master status 报告 live；collab context 显示 role=master、identity_valid=true、endpoint_live=true；collab status --all 中本 peer presence=present。
- 跑 appsdk goal subscribe --goal docs/goals/<feature>-plan.md --interval 10m（仅 master 可注册）；appsdk goal status --json 必须 active=true、desired=subscribed、observed=subscribed、collab_subscribed=true、subscription_id 非空。再做一次短间隔 live replay：armed → fired → consumed 三段证据都写入 journal/mailbox。

验证：
- appsdk verify 返回 0 且 verify evidence 写入 .appsdk/records/。
- ~/.appsdk/projects.jsonl、runtimes.jsonl、communication.jsonl 含本次 master 注册（worker_id 与 journal 完全一致）。
- ~/.collab/routes.jsonl 含 canonical_root=<abs project root> 行；.agent-collab/server/journal.jsonl 含上述五个事件行。
- appsdk goal status --json 输出符合 Long-Horizon Goal Subscription & Master Saturation 字段。

完成标准：
- 上述每条都有 source、test、live replay 三类证据。
- master 能通过 collab sendmessage/inbox/recv 双向通信；缺失信号必须显式失败而不是 fallback 或沉默降级。
- 如未取得用户对当前 peer 的 master 授权，立刻停下报告，不替用户做决定。
```

## Ordinary peer init (project already has .appsdk/project.json and a live master)

```text
/goal
目标：作为普通 peer 加入已初始化的项目，仅完成接入与自检，不接管 master 权限，不重复注册 collab。

实现文档：
- skills/appsdk-project-governance/references/bootstrap-migration.md#master-and-ordinary-peer-bootstrap
- ~/.agents/skills/collab/references/state-paths.md（Registration verification checklist）

执行规范：
- cd 到项目根；若 .appsdk/project.json 不存在或仍含 change-me/app-core，向 master 上报 placeholders，不要自补。
- 必须从项目主仓库根目录运行 `appsdk init .`，禁止从 worktree 假装注册；Collab 拒绝时会报 `collab init must run from the project main tree`，遇到该错误先 `cd` 回 canonical_root 再继续。
- 直接跑 appsdk init . 续约 AppSDK Bundle；不要新建 collab init，Collab 复用现有 daemon。
- collab context 读取当前 peer/role contract；role != master 才继续；若是 master 报告冲突并停下。
- rg -n "change-me|goal-change-me|app-core" .appsdk 应为空；否则向 master 上报。
- 按 Registration verification checklist 核对 <project>/.agent-collab/server/journal.jsonl 的 worker_id 与 ~/.collab/routes.jsonl 的 canonical_root 同主项目一致；缺一立即停下，不要伪造已注册。
- collab status --all / collab who / collab worker status <self> 检查 presence、transport、identity_valid、endpoint_live 全部 OK；否则上报 master。
- 不运行 appsdk goal subscribe，不接 master promote，不订阅 deadline 或 direct-message 之外的 lease。
- 完成首轮 sendmessage/inbox/recv 自检到 master，证据写入 .agent-collab/server/journal.jsonl。

验证：
- collab context 显示 role=peer、identity_valid=true；collab worker status <self> 报告 endpoint_live=true。
- .agent-collab/mailbox 含发给本 peer 的最近 self-check 通知，state=read，且未破坏既有条目。
- 没有任何 master promote、goal subscribe 或自主注册订阅的痕迹。

完成标准：
- 仅完成接入；不主张新 governance 根；master 与其余 peer 通过 collab sendmessage 仍能识别本 peer。
```

## Stale route cleanup (fresh start instead of migration)

```sh
collab down
cp ~/.collab/routes.jsonl ~/.collab/routes.jsonl.before-stale-cleanup-$(date +%Y%m%d-%H%M%S)
grep -v '/abs/path/missing/canonical/root' ~/.collab/routes.jsonl > ~/.collab/routes.jsonl.tmp
mv ~/.collab/routes.jsonl.tmp ~/.collab/routes.jsonl
collab up
collab status --all
```

Only use this when the daemon itself reports `HOST_ROUTE_REPLAY_FAILED: canonical root <path>: No such file or directory`. Always keep the dated backup.

## RUNTIME_BINDING_REJECTED recovery

If `collab` returns `RUNTIME_BINDING_REJECTED: pane ownership for %<n> is unknown`, the current shell is not bound to a registered pane/thread; this is not a daemon failure. Recovery:

1. Confirm `collab status --all` still reports the host daemon up.
2. Run from the original registered tmux pane or App Server thread where the peer was bound; do not run from a detached shell.
3. If the original pane is gone, recover with `collab worker recover` inside a fresh live pane and re-bind the same `worker_id`/`token`.
4. Never retry or rename the pane; never bypass by hand-editing `~/.collab/routes.jsonl` or `server.pid`. The CLI failure is the truth.
