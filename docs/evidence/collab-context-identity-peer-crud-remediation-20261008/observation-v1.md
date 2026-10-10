# 最新 observation：collab-context-identity-peer-crud-remediation-20261008

时间基准：2026-10-08 America/Los_Angeles。执行宿主：Codex Desktop。当前阶段 T0，产品代码未改。

## 最新用户目标
任务真源：/Users/fanzhang/Documents/github/appsdk/docs/goals/collab-context-identity-peer-crud-remediation-20261008.md。用户已启动执行该 Goal。
四项全部修复：context 一键身份/注册与准确补交；master/peer 用户批准覆盖恢复；master peer CRUD；context/Skill 精确操作。另修 F1 空成功。完整 installed/live/review/集成/cleanup 不能缩减。

## 版本与资源
- 主树 /Users/fanzhang/Documents/github/appsdk，HEAD 2cac9e935944bddcb98fb6e7af4beb95966dcff9，dirty docs/collab.md 和其他任务的 user-requirement-truth-lock 证据全部保留。
- git fetch origin 已成功，origin/main 最新 3dfdaf8503b7a6f6a76651a1e282c038b6648c3a。
- 本任务唯一源码 worktree /Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008，branch codex/collab-context-identity-peer-crud-20261008，从最新 origin/main 创建。
- 其他现有 worktree：baseline-flake-check-20261007、collab-master-authority-fix-impl-20261007、collab-master-presence-20261007、collab-worker-close-missing-20261008。不得修改/清理。
- 独占记录目录 /Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008。本任务新的永久证据 /Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/docs/evidence/collab-context-identity-peer-crud-remediation-20261008。
- 当前 enabled tools 未提供 native list/read/send/wait_threads 或 MCPX。没有经实时核实可用的执行 peer，使用 codex exec；不得用待修复 subagent 写命令编排。
- /opt/homebrew/bin/codex version 0.161.0；/opt/homebrew/bin/tmux 已装；/Users/fanzhang/.cargo/bin/dagpipe 已装。
- 当前 canonical /Users/fanzhang/.cargo/bin/collab version 0.2.0258；同路径 collab-mcp。
- ps 确认 canonical daemon PID 35786，命令 /Users/fanzhang/.cargo/bin/collab serve，仍运行。不将这些进程用于本轮隔离验证，不修改生产状态。
- Python socket connect 对 ~/.collab/server.sock 与 ~/.codex/app-server-control/app-server-control.sock 均成功。只证明可连接，未调用当前 Desktop thread 或生成身份；旧报告“socket 拒连”已不能作当前事实。
- 本 Desktop caller 自动观察 CODEX_SESSION_ID/THREAD_ID 均为 01a11e24-4861-79a2-ba0b-4888cce84615，host marker Codex Desktop，无 tmux/DSH anchor。仅是诊断观测，不是身份批准。

## 新事实与反证
PR #17 (1985ff26 / merge 3dfdaf85) 已增加 handle_worker_close 对 IdentityPresence::Missing 且无 unfinished task 的关闭路径，不再必须 snapshot；旧审计 F3 “关闭全部依赖不可产出 snapshot”已过宽。
保留这项合法修复。Present/Unknown 目标仍要求 snapshot，handle_worker_snapshot 仍始终 unsupported；创建缺失、真实 managed runtime close 尚未闭合。需要核对并保留 unfinished task/no-self-close 等合同。

本轮安装公开入口：
- collab subagent close audit-nonexistent-01a11e24：exit 0，stdout/stderr 空，F1 仍真实。
- collab subagent start：exit 1，MANAGED_SUBAGENT_UNSUPPORTED: tmux cannot create a Codex thread; start the peer in its own tmux pane and register that pane。
首次偏离：Cmd::Subagent 对 Start 拒绝、List/Status 观察，其余到 Ok(())，不派发 Req::Subagent。
F2 源码尚无批准 identity 路径，provide 四 scalars，客户端 merge_supplied_fact 先判冲突，master promote 前置 me()。
F4 operations 仍职责文字，缺 CRUD 参数/前置条件。

## 实现观察
identity_context host owner → resolve_for_daemon_with_route_at → Register → persist_registration → Context；当前完整策略错误路径与用户批准裁决未闭合。
生产 codex_app_server adapter 有 verify_candidate、thread/start、thread/archive helper。subagent launch 位于 cfg(test)。
注意 server notification sink/worker status/archive 存在 tmux-only unsupported 分支：静态函数存在不等于正式 runtime 可用。需真实调用确认，不能将 cfg(test) helper 解除标记后就宣称 production PASS。
生产 tmux/Native/DSH 支持说明互相矛盾；请 planner 依据实际链路冻结能力表及最小补链，不把文档改“不支持”来删本次功能。
创建/更新/关闭的具体协议尚未设计，worker runtime ownership 和幂等机制需原证据验证。

## DAG / 合同
已有 docs/dagpipe/collab-context.graph.json、collab-master-authority.graph.json、collab-pane-route-reconcile.graph.json、subscription 图与 docs/dagpipe/manifest.json。
本轮尚未 graph validate 或设计 review，不标 PASS。先按用户语义校正，不画代码调用图。
保留 docs/design/collab-master-authority-contract-20261007.md 的 grant Empty/Assigned、scope、批准替换不依赖 liveness。
旧 anchor 文档“只有 master 归属裁决”需按新要求修唯一正文。

## 事实/判断/未知
事实：以上 git、installed CLI、socket、源码入口和 PR17 改动已观测。
父判断：F1 是最先可交付的增量；复用现有 identity/control owner，避免新注册表。该判断可由 planner 更正。
未知：实际支持宿主矩阵、可工作的创建 API/选定 profile、更新合法字段、原凭据失效批准裁决的最小可靠事务、close 必要证据。缺关键信息先规划最小观测，不猜 READY。
已有安装版本与最新 HEAD 未建立来源等价性。创建/恢复/CRUD 成功未做 live；mock suite 不能代替实际 runtime。

## 授权与失败边界
已授权任务范围内开发、测试、正式本地安装/维护、review 和常规 Git 交付；正式身份 takeover 仍需具体实例批准，不自动晋升 Collab master。
不得清生产 identity/journal/reset，不能用 fallback/mock 成功。
MCPX/native thread tools 未加载，采用宿主工具记录证据；不为缺辅助工具停未受影响主线。
本轮 lsof+多读命令被自动审查误判为 process-kill chain，未执行；已用 ps 和 Python socket 读观测完成该部分，不重试该链。

下一步：独立 planner 依据此观察及引用原证据给出整体目标/首个可用增量计划、真实能力确认步骤、owner/接口/图/黑盒/交付依赖。没有独立实施计划或实现。

