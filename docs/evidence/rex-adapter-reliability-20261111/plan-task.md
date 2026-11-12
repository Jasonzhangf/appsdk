目标：修复 Collab Rex 适配的四个实际行为问题：文本与 Enter 不应因两次调用而丢失提交；Rex worker snapshot 应通过官方 capture 有界读取；Rex 通知拒绝应使用 Rex 专用错误标签；Rex idle、done、error、未知状态和过期 owner 不应被显示为 working。完成后完成独立 review、提交、PR、合并到 origin/main 和自有资源清理。

依据：
- 项目需求 `.appsdk/requirements.json` 当前有效条目 `req-minimal-governance`、`req-collab-communication-notifications`、`req-collab-identity` 和 `req-command-logs`。关键约束是最小必要机制、按真实回执区分入队/接受/消费、Rex 普通身份与 terminal endpoint 分离、后台状态感知、命令和调用日志可审计。
- 当前任务笔记：`docs/evidence/rex-adapter-reliability-20261111/note.md`。
- 现有 DAG：`docs/dagpipe/collab-rex-adaptor.graph.json`。其描述仍要求 write 后单独 Enter，和用户报告及真实探针冲突。
- Rex API revision 11 已核对：write 只排队；capture 读取 screen/scrollback；program_status 的 owner 包含 pid/name，未知 state 按 idle 处理；process 提供当前 foreground pid/name。
- 当前代码入口：`collab/src/adapters/rex.rs`、`collab/src/server/rex_monitor.rs`、`collab/src/server/mod_parts/part_05.rs`、`collab/src/server/mod_parts/part_07.rs`。

当前状态：新 worktree `/Volumes/Intel/playground/appsdk/rex-adapter-reliability-20261111`，基线 `origin/main a2951311`，干净。无产品修改。仅完成 API、源码和隔离 Enter 探针观察。仓库当前没有匹配 `codex bridge` 的源码文本。

资源与约束：
- 不运行 `collab context`，不改 identity-map、daemon、订阅、tmux 或控制状态。
- 不运行 `collab recv`、ACK、mailbox cleanup/deliver/close 或广谱 Collab 测试。
- 禁止 broad process kill、批量 session termination 和任何 `tmux kill-server`。
- 保持主树和其他 worktree 的未交付内容。
- 聚焦验证使用独立测试 fixture 或显式隔离 Rex target，避免制造通知风暴。

角色与访问：独立只读 Planner。可读上述绝对路径及项目 AGENTS、`.appsdk/requirements.json`、相关 Collab 源码/测试、`coding-principals`、`dagpipe-runtime` 与 `appsdk-project-governance`。不写产品、不安装/重启、不改配置/权限、不集成、不派 agent。

详细程度：形成执行 worker 无需猜测的计划。明确 ontology 与 DAG 修订、失败模式、红测、唯一 owner 实现、聚焦测试、真实入口 black-box、独立 review 和合并顺序。特别说明：
- 原子 Enter 的正确输入序列、receipt 语义和反例；
- capture snapshot 的输出形状、行数边界、副作用边界和失败终点；
- error label 的唯一映射 owner；
- program_status 多记录、owner 过期、idle/unknown、blocked/question、working 的优先级；
- 哪些现有测试需要改写或删除，避免保留旧错误合同；
- 不得用 terminal.write 的 `\r` 替代 capture，也不得把程序状态当作身份、接收或消费证明。

执行：本次范围内规划已授权。按本合同形成计划；缺关键事实或权限时回报具体缺口及受影响节点，不自行升权。

交付：返回完整 Markdown 正文，由宿主保存到 `/Volumes/Intel/playground/appsdk/rex-adapter-reliability-20261111/docs/evidence/rex-adapter-reliability-20261111/plan.md`；不只返回摘要、路径或“已保存”。
