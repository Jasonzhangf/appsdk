# Collab 身份恢复：当前项目与 session

本文件是身份选择的唯一设计正文。2026-10-20 用户明确要求以当前 cwd 和 tmux session 为准，替换历史 pane 归属、歧义拒绝和多重批准流程。实现入口是 `identity_resolver.rs::resolve_for_daemon_with_route_at`，提交仍由现有身份和 master authority owner 完成。

## 默认 context

`collab context` 读取当前 cwd 所属 project 和当前 tmux session。历史 pane 绑定到哪个 worker 或 project 都不能改变当前 project，也不能阻断恢复。已有 worktree 的项目映射可用于解析 cwd；worktree 中的任务和工作内容不参与身份准入。

tmux session 由观测到的 socket/server 和 `tmux_session_id` 区分。pane、pane PID、Codex thread、旧任务和旧角色不是身份选择依据。同 project/session 已有身份直接复用，切换 pane 只更新投递地址。历史额外记录不产生新的用户裁决门禁。

| 当前状态 | context 的结果 |
| --- | --- |
| 当前 project 没有 master | 自动恢复或建立当前 session 的 peer，不申请批准 |
| 其他身份是 master | peer 恢复成功，并给用户一个可选的 master 替换选择；未授权时 peer 仍可使用 |
| 当前 project/session 已是 master | 复用身份与现有授权，不重复询问 |
| 用户明确授权当前身份恢复为 master | 读取当前身份与 incumbent，通过既有 authority owner 直接替换 master |

master 替换不要求调用方提供旧 token、旧 target snapshot、intent digest、lease 或 grant generation。当前事实由 daemon 读取。现有 `approve_grant` 可承载 `action=restore_master` 与用户授权原文；不要求第二份身份批准。未获 master 授权不使普通 peer 恢复失败。

## 持久化和通信

有效身份复用已有身份记录和 token，并同步恢复当前通信端点、route 与收信订阅。切换 pane 时更新所有投递事实，旧 route 自动失效。首次建立身份仍通过原 Register owner 提交真实记录。相同操作的回放复用已提交结果，不重复 master 转移。身份、master 授权、消息、任务各自由现有 owner 管理，不手工编辑 journal 或文件制造恢复结果。

身份恢复不检查工作内容、任务、claim、worktree、master 派单或历史 pane 所有权。恢复的结果必须可通信，不能仅凭角色字段宣称完成。实际持久化、端点或 I/O 错误保留原错，不伪造成功；它们不授权 agent 操作 daemon 生命周期。恢复验收通过真实发送、接收和回复完成，不增加用户必须手动执行的通信恢复步骤。

Collab 是一个全局、由用户管理的 daemon。agent 只连接现有服务。服务不可用时保留身份与任务，返回服务不可用。维护命令和凭据不属于 agent 接口，身份恢复不申请维护窗口。

## 其他宿主

非 tmux 的 AppServer 仍使用其观测到的 session/thread，DSH 使用 `DSH_SESSION_ID`。缺失事实只按实际 adapter 所需字段补充一次。不借其他宿主的历史 pane 选择身份，不要求用户额外证明历史身份或完成第二次恢复。

## 验收边界

公开 `context` 用例覆盖：旧 pane 属于 A 时当前 cwd B 仍恢复为 B 的 peer；同 project/session 换 pane 保持身份；已有 master 时 peer 可用；一次用户授权直接替换 master；重复 context 不再询问或 Register；daemon PID 不变。peer 恢复与 master 恢复分别实际发送、接收并回复，检查消费回执。项目任务与旧记录保留，不成为身份恢复的拒绝条件。

外层 operation/query/cancellation 的持久化实现见 [context 合同](collab-identity-context-recovery-contract-20261008.md)。其中历史 pane/fence 设计不覆盖本文件的当前身份选择规则。图入口为 `docs/dagpipe/collab-context.graph.json` 与独立 master authority 图。
