# Merge-pending state machine（按 DAGpipe 规范）

**本页是审计图，不是函数调用链。** 业务入口是 `collab task review --accept`；
成功沉没点是 `task closed`。失败终点显式报错，不允许回边。机器可校验的 SESE
图见 `docs/dagpipe/merge-pending.graph.json`（`dagpipe graph validate`）。

## 背景：被修复的开环

旧链路里 `task review --accept` 把任务置为 `accepted`，然后靠一条
`collab sendmessage` 通知 master merge。master 忙时这条消息只是一条普通通知，
没有任何持久 obligation，也没有 close 门禁，所以 merge 会被遗忘，任务停在
`accepted`。这是开环：通知的消费者（master）不在闭环里，且没有失败终点。

当前 lifecycle 先记录显式 delivery kind。代码交付进入适用的 pending merge
闭环；`task deliver --no-candidate` 的只读交付在 review accept 后到达
`no_merge_required`，不创建 integration obligation，但仍须完成 owned-resource
cleanup 才能正常关闭。缺失 candidate 不会被推断为只读。
只读交付仍须提供请求的实际结果和适用验收证据，完成真实消息消费、资源清理与
正常 close。读取、ACK、摘要或派单回执不能代替完成。

## 单源单汇

| 角色 | 唯一 owner | 说明 |
| --- | --- | --- |
| delivery kind 真源 | daemon journal (`TaskLifecycleRecord.delivery_kind`) | 仅显式 `--no-candidate` 记录 read-only；缺失 candidate 默认为 code |
| merge obligation 真源 | daemon journal (`Event::MergeRequested` / `MergeResolved`) | 仅适用于 code delivery；不是聊天消息，不是 master 记忆 |
| 登记触发 | code delivery 的 `handle_task_review` accept 分支，且存在 live master | 与 `accepted` 状态同一事务提交；read-only 不登记；无 live master 的 code delivery 保持 exact-candidate owner 自集成生命周期 |
| 解除触发 | live master 的 `task integrated`、task owner 的 accepted→rework、cancel / authorized force close | 与对应状态转移同一事务提交；pending 未解除时 owner 的 integrated/close 返回 `TASK_MERGE_PENDING` |
| 关闭门禁 | `handle_task_close` | pending 未解除时返回 `TASK_MERGE_PENDING` |
| 提醒投影 | `collab context` / `collab status --all` / `appsdk longhorizon show` / master idle wake | 只读投影，不写第二份真源 |

## 事件表

| 事件 | 生产者 | 消费者 | 何时发生 | 状态效果 | payload 边界 |
| --- | --- | --- | --- | --- | --- |
| `review_accept_code` | task owner 或 live master | daemon | code delivery 的 `task review --accept`（需存在 live master） | `accepted` + `MergeRequested` | task id、evidence、delivery kind |
| `review_accept_read_only` | task owner 或 live master | daemon | 显式 read-only 的 `task review --accept` | `accepted` + `no_merge_required` | task id、evidence、delivery kind |
| `merge_pending_notice` | daemon | live master | 登记同一事务 | durable direct-message `merge-pending:<task>` | task id、owner |
| `idle_wake` | keepalive/timer | live master | master idle 且有 pending | body 含 `pending_merges=` | 只读 |
| `integration_recorded` | live master，或无 master 时的 owner | daemon | code delivery 的 `task integrated --commit`；provided commit 与 exact delivered candidate 均可达 main | `merged` + `MergeResolved` | commit、evidence |
| `review_rework` | 有权 review 的 task owner 或 live master | daemon | 仅 `task review --rework`，输入为 `delivered` | `rework` + `MergeResolved`（若存在 pending） | evidence |
| `accepted_owner_rework` | task owner | daemon | accepted 任务由 owner 执行 `task update --status rework` | `rework` + `MergeResolved` | evidence |
| `force_close` | 授权方 | daemon | 授权 force close | `closed` + `MergeResolved` | reason |
| `close_blocked` | daemon | caller | pending 未解除时 close | 失败 `TASK_MERGE_PENDING` | task id、rule |

## 状态机

```mermaid
stateDiagram-v2
    state "已交付" as delivered
    state "已验收" as accepted
    state "无需合并" as no_merge_required
    state "待清理资源" as cleanup_pending
    state "已集成" as merged
    state "待返工" as rework
    state "已关闭" as closed
    [*] --> delivered : 提交交付
    delivered --> accepted : 代码验收并登记适用合并义务
    delivered --> accepted : 显式只读交付验收
    accepted --> no_merge_required : 只读交付无需合并
    no_merge_required --> cleanup_pending : 清理自有资源
    cleanup_pending --> closed : 核验清理回执后关闭
    delivered --> rework : 交付审查要求返工（review --rework）
    accepted --> merged : 代码候选可达 main 后记录集成并解除合并义务
    accepted --> rework : owner 更新为返工并解除合并义务
    accepted --> closed : 明确授权强制关闭并解除合并义务
    accepted --> 拒绝关闭 : 合并义务未解除时请求关闭
    拒绝关闭 --> [*] : TASK_MERGE_PENDING（任务仍为 accepted）
    merged --> closed : 核验清理回执后关闭
    closed --> [*]
    rework --> [*]
```

## 终点检查

| 终点 | 检查方法 |
| --- | --- |
| merge obligation 持久 | 重启 daemon 后 `collab status --all` 仍含该 task id |
| read-only terminal 持久 | replay 后 `delivery.kind=read_only`、`disposition=no_merge_required`，无 pending merge |
| master 可见 | `collab context` 的 `operations` 含 `merge_pending` |
| 长程可见 | `appsdk longhorizon show` 的 `待合并` 区块列出该 task |
| idle 提醒 | master idle wake body 含 `pending_merges=<task>` |
| close 门禁 | pending 期间 `task close` 返回 `TASK_MERGE_PENDING` |
| 正确解除 | `task integrated` 后 `pending_merges` 不再含该 task，且可 close |

## 失败终点

| 失败 | 语义 | 处理 |
| --- | --- | --- |
| read-only 调用 integrated | `TASK_NO_MERGE_REQUIRED` | 完成 cleanup 后正常 close |
| exact code candidate 未解析 | `TASK_DELIVERY_CANDIDATE_UNRESOLVED` | rework 后用 code intent 和可解析 worktree 重新 deliver |
| commit 不在 main | `TASK_INTEGRATION_COMMIT_MISMATCH` | 先真正 merge，再记录 |
| pending 期间 close | `TASK_MERGE_PENDING` | 由 master 完成 merge 与 integrated |
| pending 期间 owner 自 integrated | `TASK_MERGE_PENDING` | 不能绕过 master 义务；必须先由 master merge 并记录 integrated |
| delivered review rework | `task review --rework` 只接受 delivered | 转为 `rework`，并显式解除既有 obligation，不残留 pending |
| accepted owner rework | task owner 以现有 `task update --status rework` 提交 | 与 `MergeResolved` 同事务；master 不能借此获得 owner-only update 权限 |
| 授权 force close | obligation 显式解除并留 reason | 审计可查 |
