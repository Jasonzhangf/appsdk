# AppSDK + Collab Initialization Prompts

These prompts are the entry point for `appsdk init`/`collab init`. The
single self-check line that `appsdk init .` prints IS the registration
result (worker_id, role, transport, identity_valid, endpoint_live,
subscription). Do not pre-probe with `collab who`, `whoami`, `pane`,
`routes.jsonl`, or `.agent-collab/*`; only fall back to the detailed
verification checklist in
`~/.agents/skills/collab/references/state-paths.md` when the self-check
fails.

Registration must run from the project main tree (canonical root), not
from a `playground/<slug>` worktree. The daemon rejects any init that
starts in a worktree with `must run from the project main tree`.

## Master init (after user approval for the exact project + peer)

```text
/goal
目标：在当前项目根完成 AppSDK + Collab 的全新 master 初始化，由 `appsdk init .` 一次性返回当前状态/能力/反馈；不允许额外探活。

实现文档：
- skills/appsdk-project-governance/SKILL.md
- skills/appsdk-project-governance/references/bootstrap-migration.md
- skills/appsdk-project-governance/references/state-paths.md
- ~/.agents/skills/collab/SKILL.md
- ~/.agents/skills/collab/references/state-paths.md

执行规范：
- cd 到主仓库根目录；worktree 不接受注册，报错时先回到 canonical_root。
- 直接 `appsdk init .`，把它的 self-check 行当作回执：worker_id、role、transport.kind、identity_valid、endpoint_live、订阅状态。**不要先跑** collab who / whoami / pane 探测 / routes.jsonl / .agent-collab/*。
- 前序控制面已清理：删除/保留 .appsdk、.appsdk-control、.agent-collab；只保留全局 ~/.appsdk、~/.collab 真源。如果旧控制面还在，停下先完成 fresh-start 步骤。
- appsdk prepare → 确认 .appsdk-prepare.json 的 status="confirmed"、change_kind="new_project"、confirmed_by/confirmed_at 已填写；未确认先停下询问用户。
- 按真实 contract 改写 .appsdk/project.json / .appsdk/goal.json（占位符 change-me / goal-change-me / app-core 必须替换）。
- appsdk guide compile + appsdk verify；rg -n "change-me|goal-change-me|app-core" .appsdk 必须无命中。
- self-check 通过后，用户显式批准当前 peer 作 master，再 `collab master promote --approval "<用户原文>"`；未批准前不要 promote。
- appsdk goal subscribe --goal docs/goals/<feature>-plan.md --interval 10m（仅 master）；appsdk goal status --json 必须 active=true、observed=subscribed、collab_subscribed=true、subscription_id 非空。再做一次短间隔 live replay：armed → fired → consumed 三段证据都写入 journal/mailbox。

验证：
- appsdk verify 退出 0 且 evidence 写入 .appsdk/records/。
- ~/.appsdk/{projects,runtimes,communication}.jsonl 含本次 master 注册；worker_id 与 self-check 一致。
- ~/.collab/routes.jsonl 含 canonical_root=<abs project root>；.agent-collab/server/journal.jsonl 含 GlobalProjectRegistered、GlobalRuntimeBound、Registered、NotificationSubscribed、CommandCompleted 五事件。

完成标准：
- 上述每条都有 source、test、live replay 三类证据。
- master 能通过 collab sendmessage/inbox/recv 双向通信；缺失信号必须显式失败而不是 fallback。
- 如未取得用户对当前 peer 的 master 授权，立刻停下报告。
```

## Ordinary peer init (project already has .appsdk/project.json and a live master)

```text
/goal
目标：作为普通 peer 加入已初始化的项目，`appsdk init .` 一次性回执当前状态；不要重复注册 collab，也不要接管 master 权限。

实现文档：
- skills/appsdk-project-governance/references/bootstrap-migration.md#master-and-ordinary-peer-bootstrap
- ~/.agents/skills/collab/references/state-paths.md

执行规范：
- cd 到主仓库根目录；worktree 不接受注册，遇到 `must run from the project main tree` 先回 canonical_root。
- 直接 `appsdk init .`；self-check 已直接给出 worker_id、role=peer、identity_valid、endpoint_live、订阅状态。不要在成功响应后再跑 collab who / whoami / pane / routes.jsonl / .agent-collab/*。
- 如 .appsdk/project.json 不存在或仍含 change-me/app-core，向 master 上报 placeholders，不要自补。
- 不运行 appsdk goal subscribe，不接 master promote，不订阅 deadline 或 direct-message 之外的 lease。
- 完成首轮 sendmessage/inbox/recv 自检到 master，证据写入 .agent-collab/server/journal.jsonl。

验证：
- self-check 中 role=peer、identity_valid=true、endpoint_live=true。
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

If `collab` returns `RUNTIME_BINDING_REJECTED: pane ownership for %<n> is unknown`, the current shell is not bound to the registered pane/thread; this is not a daemon failure. Recovery:

1. Confirm `collab status --all` still reports the host daemon up.
2. Run from the original registered tmux pane or App Server thread where the peer was bound; do not run from a detached shell.
3. If the original pane is gone, recover with `collab worker recover` inside a fresh live pane and re-bind the same `worker_id`/`token`.
4. Never retry or rename the pane; never bypass by hand-editing `~/.collab/routes.jsonl` or `server.pid`. The CLI failure is the truth.
