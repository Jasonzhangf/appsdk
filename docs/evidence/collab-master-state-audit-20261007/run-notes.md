# Collab master 状态审计节点笔记

范围：只读诊断和设计重画。未授权执行本轮 master 切换、reset、安装或 daemon 生命周期动作。保留已有 dirty 文件。

- 2026-10-07 基线｜完成｜当前 main，HEAD `7350fbf6b020b1464d531337c7b2f6b8fa5de6f2`；原有 `docs/collab.md` 修改和 requirement-truth-lock evidence 未改｜源码输入固定；runtime 可变｜查实际入口。
- 入口与历史｜完成｜`rust/src/main/init.rs:842-899`、`docs/dagpipe/collab-master-authority.graph.json`、既有 shortest-path 设计｜普通 init 调 collab init；fresh init 在 governance reset 后提前返回；均无 master authority 清除调用｜查存活判定。
- 首次偏离｜源码确认｜`collab/src/server/mod_parts/part_01.rs:275-285`、`part_07.rs:1-65,403-425,1198-1267`｜生产 oracle 对 Tmux 固定返回 `TMUX_ENDPOINT_NOT_QUERYABLE`；现存 pane 且有 IDs 的 Tmux worker 仍映射 Unknown；master 操作要求 Present；显式 promote 已不检查旧 master 存活｜查公开诊断。
- 公开诊断｜完成｜已安装 `collab --version` 为 `0.2.0253`；AppSDK cwd `collab master status` exit 0，master=null、recorded_unusable=null；routecodex cwd同命令 exit 1，`master identity is unknown; defer authority changes until transport probes succeed`，recorded_worker_id=`codex-%2`、status=unknown｜不能把 AppSDK cwd 无 master 外推到其他项目；未做状态写入实验｜查 panel 和恢复门。
- panel｜源码确认、当前 UI 未验｜`board_handlers.rs:50-88`、`dashboard/app.js:59-61`、`dashboard.rs`｜role 来自授权记录；在线状态来自 worker_presence；panel HTTP 仅 GET；`collab board show` 在本会话 exit 1 `BOARD_IDENTITY_REQUIRED`｜不为了审计注册新 peer，不宣称看到了用户 panel｜画语义图。
- 状态耦合｜源码确认｜`part_06.rs:1101-1115,1264-1383`、`global_state_impl_part2.rs:600-604,752-858`｜注册仍含 master live/unknown 门；授权绑 runtime binding generation；同项目按 app scope 排他；legacy master 字段仍用于部分投影｜准备消融清单和分离状态机。
- 设计重画｜完成｜`docs/design/collab-master-state-audit-20261007.md`、本目录两个 graph.json｜当前行为与提议明确分开；无产品代码修改；不作实现准入或功能修复 PASS｜静态图校验。
- 静态校验｜PASS｜current graph：5 nodes/4 edges/5 waves；proposed graph：3 nodes/2 edges/3 waves；现有 master-authority graph：3 nodes/2 edges/3 waves；三次 `dagpipe graph validate` exit 0；`git diff --check` exit 0｜只证明图拓扑和 binding 形状，不证明 runtime 或提议行为｜交付审计报告，保留本轮文档产物。

本轮新增知识：问题不是缺少一个 force flag。当前显式转移已绕过存活，但授权执行、context 投影和注册恢复继续混合授权与可达性，形成成功后不可用的死边。依据为上述源码与公开诊断。归属当前审计报告；未写长期记忆或全局规则。
