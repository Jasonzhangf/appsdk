# 第一轮 observation

任务/输入：collab-master-authority-fix-20261007；源码base/main/origin main=7350fbf6b020b1464d531337c7b2f6b8fa5de6f2。独占代码worktree=/Volumes/Intel/playground/appsdk/collab-master-authority-fix-20261007；branch=codex/collab-master-authority-fix-20261007。

用户目标：旧错误master无法通过重新初始化清除，tmux下master存活判断错误且不能覆盖；要求重画、简化状态机。随后『派gcm worker解决问题 合并到main，重建重启』。已授权本项目安装、正式down/up维护中断、测试、review、commit、main集成/push。新增全局规则要求本轮独立oauth/gpt-6.1-sol planner；实现者仍GCM。

当前行为与同入口复现：installed collab 0.2.0253。AppSDK cwd master status exit0，master=null/recorded_unusable=null；RouteCodex cwd collab master status exit1，recorded_worker_id=codex-%2、status=unknown，原错『master identity is unknown; defer authority changes until transport probes succeed』。当前会话 board show exit1 BOARD_IDENTITY_REQUIRED，不为了观察注册身份。用户当时panel实际响应未取得，不能宣称显示在线。

源码观测：collab/src/server/mod_parts/part_01.rs:275 的production oracle对Tmux固定返回TMUX_ENDPOINT_NOT_QUERYABLE；part_07.rs:1-65的Tmux presence有IDs时调该oracle，无IDs时直接Unknown；part_07.rs:403及1198的master权限操作要求Present；board_handlers.rs:133有同样live门。part_07.rs:1224的promote已允许明确批准原子替换旧grant且不probe；part_06.rs:1044-1115的注册恢复仍有master live/unknown判断和same-pane/same-DSH例外。part_09.rs:1281投影按presence把授权拆成master/null/unknown/recorded_unusable。board_handlers.rs:50-88 role来自typedgrant、status来自presence。dashboard.rs HTTP只有GET。global_state_impl_part2.rs:600-604重绑撤grant，752-858授权绑定generation并按project+appscope排他。part_10.rs project_route_actor已校验token/scope/currentbinding/generation。

AppSDK init观测：rust/src/main/init.rs:842-899普通初始化调用collab init；fresh governance init提前return；都无master revoke语义。初始化不应暗删协作任务消息，明确清除授权需要唯一owner操作。

能力与证据：cargo、git、git-bug、gh、codex、dagpipe、tmux可用。base上cargo test --manifest-path collab/Cargo.toml --test master_status_cli exit0、2passed，日志baseline-master-status.log。已有collab/tests/tmux_recv_e2e.rs真实CLI/隔离daemon/tmux和appserver_two_tui_integration.rs consumer。正式installer scripts/install-global-collab.sh委派collab/scripts/install-global-collab.sh，自动全局版本号、安装canonicalCLI/MCP与技能，验binary/skill相等。正式runtime服务已存在，不得改为临时candidate路径。

DAG：主树已有docs/dagpipe/collab-master-authority.graph.json（批准替换3节点），collab-context.graph.json、collab-dashboard.graph.json。上轮审计current/proposed graphs都dagpipe validate exit0，只有拓扑形状证据；审计产物在本记录目录audit.md、audit-evidence/，并非实现准入PASS或可执行Operator。

任务状态：parent-notes.md为父任务状态；git-bug1f79966 OPEN。独立GCM设计reviewer正在只读审核，结果未出；runtime worker正在预检；GCM implementation只开始独立预备读取，产品写入前必须等待准入。无产品代码修改（design reviewer有本任务notes.md临时文件误写worktree根，parent待其退出后归档，不集成）。

运行观测/未知：runtime worker通过lsof报告正式socket由PID42599拥有，loadedpath为canonical collab、cwd是RouteCodex；descriptor service_scope_root是AppSDK、desired_state=down，official status报DAEMON_UNKNOWN。只证明这些观察，不证明同一scope多个daemon冲突，也未证明daemon具体卡住原因。正式动作前parent需刷新精确PID/socket/descriptor，官方down/up失败先查实际结果，不能盲起第二daemon。其他既有isolated serve/tmux不清理。

变化：首次独立规划；新增规则发生在产品写入之前，需本轮oauth planner给READY。用户目标/授权未变。上轮audit是候选设计和事实，planner可收紧scope，不把可选全局schema迁移纳入小修。

资源：GCM implementation独占collab/src、collab/tests、相关docs/design/dagpipe、repo skills/collab；不能改原主树dirty docs/collab.md，不能全局写配置/技能。planner产品只读，父CLI保存plan正文。parent独占runtime维护、review、commit/merge与清理。MCPX现行registry无本候选，session open WORKSPACE_NOT_FOUND；需要重启非目标MCPX才注册，本轮不做，CLI留证据。

事实/假设/未知：tmux oracle拒绝和grant执行耦合为源码事实，公开Unknown为复现事实；用户本次所有拒绝分支未完成写入对照复现，必须新增base红测。当前project+appscope是既有隔离合同，是否应改全项目唯一属设计决定且涉及迁移，本轮优先保持并明确范围。稳定主体恢复可以复用existingtypedgrant事务，不先做全量存储迁移。安全边界是当前请求认证、旧generation拒绝、用户批准范围与独立recipientaddress验证；移除权限live门不等于允许向unknownshell发送。
