# rule-upgrade-audit-20261009 / appsdk-audit-runtime

## Task anchor

- 时间：2026-10-09
- 目标：只读审计整个 `collab/`，覆盖全部现行产品源码、Cargo、相关 tests/docs 与安装脚本接口。
- cwd：`/Users/fanzhang/Documents/github/appsdk`
- 基线：`3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`
- 硬边界：源码只读；不运行 collab context/up/down；不查看或改变 `~/.collab`；不运行测试/build；不安装/重启/commit/push；不派 agent；不改长期经验。
- 唯一可写文件：本 `notes.md`。
- 交付：CLI 将最终报告保存为 `report.md`；本轮不声称测试或产品 PASS。

## 采证节点

### 节点 1｜基线、范围与入口已读

- 状态：完成
- 证据：
  - `git rev-parse HEAD` = `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`。
  - 已读 `/Users/fanzhang/.agents/AGENTS.md`、`coding-principals/SKILL.md`、`references/workflow.md`、全局 `collab/SKILL.md` 全文。
  - 已读父 `note.md`、`collab/README.md`、`collab/Cargo.toml`、`collab/docs/{function-map,resource-map,verification-map,mainline-call-map,collab-v1-lifecycle.manifest}.json/md`。
  - `collab/` 共 154 个文件、约 97,843 行；含 68,211 行匹配测试名的源码与 23,247 行显式 test 文件，后续需按真实产品/测试边界重新分组。
- 初步观察（未定论）：
  - `resource-map.md` 与当前 `collab/SKILL.md` 对 master authority 的描述可能漂移：前者仍写 live/tmux liveness，后者写 typed grant 与 transport liveness 分离。
  - `.github/workflows/verify.yml` 当前只跑 `rust/` 与 `dagpipe/`，初步看没有 `collab/` 的 fmt/test/build gate。

### 节点 2｜DAG 与消费者重建

- 状态：进行中（证据累积中）
- 已读（本轮追加）：`src/server/mod.rs`、`mod_parts/part_01..04,11,12`、`board_handlers.rs`、`board_invites.rs`、`board_execution.rs`、`install_skills.rs`、`notification_contract.rs`、`notification_state.rs`、`presence.rs`、`identity_resolver.rs`（全）、`identity.rs`（前半）、`subagent.rs`（入口/分发段）、`main_live_closure.rs`（结构）、`main.rs` 关键分支、`main_cli.rs` TaskCmd、`bin/collab-mcp.rs` 关键段。

#### 证据 A｜`collab subagent dispatch`（及 ready/working/send/snapshot/rearm/close）在 CLI/MCP 静默 no-op（确定 finding）

- `collab/src/main.rs:356` `Cmd::Subagent` 分支：仅 `Start` 显式 bail；其余先取 `subagent_observe_query`；`List/Status` 走 `Req::SubagentObserve`；**未匹配的分支直接 `Ok(())`**（`collab/src/main.rs:365-375`）。
- `collab/src/main_context.rs:358-365` `subagent_observe_query` 只对 `List`/`Status` 返回 `Some`，其余返回 `None`。
- 因此 `dispatch/ready/working/send/snapshot/rearm/close` 落回 `Ok(())`，不构造任何请求、不打印、进程 exit 0。
- MCP：`collab/src/bin/collab-mcp.rs:217-236` `collab_subagent` 拼 argv 后 spawn CLI；`call()`（`:178-190`）在 exit 0 且 stdout 空时返回 `Ok("")`，`tools/call`（`:577-590`）以 `isError:false` 返回空 text。
- daemon 侧真实实现存在且被挂载：`src/server/mod_parts/part_11.rs:41-52` `Req::Subagent` → `crate::subagent::handle_with_env`；`src/subagent.rs:812` `handle_with_env`，`:819` `Action::Dispatch` → `handle_scheduler_dispatch`，`:862` `run()` 覆盖 List/Status/Snapshot/Rearm/Send/Ready/Working/Close。
- **生产代码中没有任何构造 `Req::Subagent` 的调用点**：`rg -n "Req::Subagent" collab/src` 只命中 `main.rs`（是 `Req::SubagentObserve`）、server 分发/准入分类、以及 tests。唯一 `Req::Subagent {` 的构造在 `host_route_registry_tests`、`peer_tests`、`scheduler_admission_tests`。
- 消费者依据：`collab/skills/collab/SKILL.md:547`（`collab subagent dispatch --request-id ...` 为 durable scheduler 派单入口）、`:659` dispatch 模板、`:529` `collab subagent ready|working <id>`；`bin/collab-mcp.rs:28` `collab_subagent` 描述与 enum 列出 dispatch/ready/working/...。
- 影响：SKILL 文档化的 master 派单主路径与受管子代理生命周期在 CLI/MCP 不可达；进程/工具返回成功，违反“错误显式、不伪造成功”。daemon 侧 `handle_scheduler_dispatch` 与 `subagent::run` 成为仅测试可达的孤儿路径。
- 最小修订：把非 `Start` 的 action 经 `Req::Subagent { worker_id, token, command, launch_env: BTreeMap::new() }` 送到 daemon（`worker_id/token` 来自 `me(&scope)`）；若产品确实不再支持，则 CLI/MCP 对该 action 显式 fail，不保留静默成功。
- 定向验证命令（本轮只读，未运行）：
  - 静态：`rg -n "subagent_observe_query|Cmd::Subagent|Req::Subagent" collab/src/main.rs collab/src/main_context.rs collab/src/server/mod_parts/part_11.rs`
  - 黑盒（需授权环境）：`collab subagent dispatch --request-id x --subject s "body"` 断言 daemon 产生 task/admission，而非无输出 exit 0。

#### 证据 B｜CI 不含 `collab/` 任何 gate（确定 finding，低风险但真实）

- `.github/workflows/verify.yml` 三个 job 仅覆盖 `rust/Cargo.toml`（fmt/test/release build）与 `dagpipe/Cargo.toml`（fmt/test）；`release` job 也只 build `rust/`。`rg -rn collab .github/` 无命中（仅 `collaboration-*` schema 文件名巧合）。
- `collab/` 是独立 Cargo 包（`collab/Cargo.toml` + 自带 `collab/Cargo.lock`），不在任何 workspace 内。
- 现有 gate 只在文档里以手写命令存在：`docs/test-design/merge-pending-dag.md`、`docs/codex-tui-collab-architecture.md`、`docs/sdk-source-limit-repair-v1.md` 均写 `cargo test/fmt/build --manifest-path collab/Cargo.toml`，无 CI 执行。
- 影响：collab fmt/test/build 无 CI 门禁；证据 A 这类入口回归不会被 CI 捕获（现有测试只覆盖辅助函数，见下）。
- 最小修订：在 `verify.yml` 增加 `collab/` 的 fmt --check 与 test（至少 `--bin collab --bin collab-mcp`）。

#### 证据 C｜docs 漂移（确定 finding，文档层）

- `collab/docs/resource-map.md:6` 仍写 master authority = “journaled `Event::MasterAssigned` plus tmux pane liveness probe”。当前实现明确 liveness 不参与授权：`src/server/mod_parts/part_07.rs:1193-1194`（promote 不探测 pane）、`:1292`（delegate 的 liveness 是独立通信事实、不否决）、`part_07.rs:831`（授权准入不咨询 liveness）；`board_handlers.rs:70-78` 把 authority 与 transport observation 分开投影。
- `docs/dagpipe/collab-context.graph.json`、`collab-dashboard.graph.json`、`collab-master-authority.graph.json` 的 `description` 结尾均写“实现代码边当前待实现”，但对应 handler 已存在：`handle_context`（`mod_parts/part_09.rs:1051`）、`handle_board_show`/`handle_board_command`（`board_handlers.rs:49/147`）、`handle_master_promote/clear/delegate`（`mod_parts/part_07.rs:1176/1230/1271`），并在 `part_11.rs:29-30/460-475` 挂载。
- 影响：规则/文档升级前的真源与实现不一致，读者会以为 master authority 仍依赖 pane liveness、或以为三条链路未实现。
- 最小修订：更新 `resource-map.md` 的 master-authority 行；移除三张 graph 描述里的“实现代码边当前待实现”，或改为指向真实 handler。

#### 证据 D｜旧 wire 变体是显式 fail-closed，不建议删除（保留理由）

- `Req::MasterRecover/TransferMaster/RemoveWorker/ResetBindings`（`proto.rs:646-667`）无 CLI/MCP 生产者，但 daemon handler 显式返回 deprecated 错误（`part_11.rs:569-584`），`TaskDispatch`/`TaskClaim` 同样显式 deprecated（`part_08.rs:1015/1026`）。属“老客户端 fail-closed 兼容”，非死代码，不应仅凭无生产者删除。

#### 证据 E｜`CrossProjectSend` 在 routed 路径可用（排除误报）

- `main.rs:698` `collab master send` 构造 `Req::CrossProjectSend` 发到 target 项目 socket。
- 该请求走 `conn_task_routed → dispatch_wire_routed → manager.dispatch_sync`；`part_04.rs:1134` 在 `dispatch_sync` 中**先**对 `CrossProjectSend` 调 `dispatch_cross_project_send`（`part_04.rs:1022`），早于 `validate_request_context`/`dispatch_with_route_context` 的拒绝分支（`part_11.rs:138`）。故 routed 路径支持跨项目发送；`part_11`/`validate_request_context` 的拒绝仅作用于非 routed/in-process 路径。

- 下一步：读 `mailbox.rs`、`adapters/mod.rs`、`scope.rs`、`config.rs`、`client.rs`、`migration*.rs`、`reset.rs`、`server/mod_parts/part_05..10,13`，以及关键 tests；核对每个 `Event` 写入者/replay 分支、dashboard 只读投影、install/version 链闭环；再写 DAG 与最终报告。

#### 证据 F｜`client::adapters` 的 `legacy` 子模块基本是死代码（确定 finding，删除候选）

- `collab/src/client.rs:1-2` `#[path = "adapters/mod.rs"] pub mod adapters;`，故 `adapters/mod.rs`（1271 行）在 binary crate 内编译，无外部 lib 消费者。
- `adapters/mod.rs:20-1270` `mod legacy { ... }` 定义 `AdapterRegistry`、`UnavailableAdapter`、`AppServerAdapter` trait、`EndpointBinding`、`AdapterSelection`、`SubmissionReceipt`、`InterruptReceipt`、`submit_registered`、`interrupt_registered`、`binding_for_request`、`binding_for_explicit_env`、`validate_endpoint_binding`、`validate_turn_matches`、`require_capability`、`timeout`、`unknown` 等。
- 消费者证据：`rg -l "\bAdapterRegistry\b|\bsubmit_registered\b|\binterrupt_registered\b|\bUnavailableAdapter\b|\bAppServerAdapter\b" collab/src` 只命中 `adapters/mod.rs` 自身（tests 也在 `adapters/mod.rs` 内）。仅 `EndpointKind`/`WakeMode`/`AdapterCapabilities`/`AdapterError`/`NotificationDeliveryClass`/`notification_class_from_display` 被 `codex_app_server_production_part1.rs`、`part_05.rs` 真正使用。
- 编译器同证：`docs/evidence/collab-master-authority-fix-20261007/candidate-install.log:452`（trait `AppServerAdapter` is never used）、`:470-476`（`AdapterRegistry`）、`:493-559`（`binding_for_request`/`submit_registered`/`interrupt_registered`/`validate_*` 等 never used）。
- 影响：约 800-1000 行未接线的“adapter registry”抽象，制造大量 `dead_code` warning，掩盖真实 warning，属最小充分结构的删除候选。
- 最小修订（需 owner 确认后再做，不在本轮）：删除 `legacy` 模块中未被消费的 registry/trait/receipt 面，仅保留 `EndpointKind`/`WakeMode`/`AdapterCapabilities`/`AdapterError`/`NotificationDeliveryClass`。定向验证：`cargo build --manifest-path collab/Cargo.toml 2>&1 | rg "never used"` 应无 `AdapterRegistry|submit_registered|interrupt_registered` 命中。

#### 证据 G｜`migration.rs` + `migration_tail.rs`（约 2850 行）无生产消费者（确定 finding，删除/接线候选）

- `collab/src/main.rs:7` `pub(crate) mod migration;`；`migration.rs:77-80` `#[path="migration_tail.rs"] mod migration_tail; pub use migration_tail::*;`。
- 消费者证据：`rg -n "\bmigration::" collab/src collab/tests` 无任何命中（除自身文件与 `migration_tests.rs`）。daemon 的迁移功能实际由 `state.rs` 的 `MigrationRecord` + `mod_parts/part_06.rs:503-673` 的 `handle_migration_inspect/plan/apply/verify` 实现，与本模块无关。
- 编译器同证：`candidate-install.log:634-899` 报告 `MappingClass`、`SourceDisposition`、`ProjectAdmission`、`MigrationPhase`、`MigrationContractError`、`ImmutableSnapshot`、`TargetEpoch`、`VerifiedPrefix`、`MigrationApplyReceipt`、`MigrationManifest`、`MigrationTransaction`、`InspectionReport`、`inspect_jsonl*` 等整片 never used。
- 影响：与真实 `MigrationRecord` 迁移链并存的第二套“未接线迁移契约”模块，重复 owner 风险 + 维护/编译负担。
- 最小修订（需 owner 确认）：要么把该 inspection/classify 面接到真实 `collab migrate` 入口，要么删除/移到未编译的 design 资产。定向验证：`rg -n "\bmigration::" collab/src` 若仍为空，则确认无接线。

#### 证据 H｜仓库 skill 参考文档两处与代码/自身不一致（确定 finding，发布前必须修）

- `collab/skills/collab/references/migration-daemon.md:55,140` 写 `collab reset --discard-legacy ...`（缺 level flag）；但 CLI `collab/src/main_cli.rs:355-372` 要求 `--project|--host`，`reset.rs:44-53` `ResetLevel::select` 在二者皆缺时 bail `RESET_LEVEL_REQUIRED`；`main.rs:770-785` 直接调用 `select(project, host)?`。同一仓库的 `references/state-paths.md:40`、`SKILL.md:247-248` 仍写 `collab reset --project/--host`，故 `migration-daemon.md` 是错误的一处。
- `collab/skills/collab/references/task-worktree-lifecycle.md:59-60`（仓库版）改写成“Last owned close cancels this peer's direct-message auto-notify”；但 `handle_task_close`（`mod_parts/part_09.rs:325-660`）只 supersede keepalive 消息、清 keepalive 计数，不发任何 `NotificationStatus` cancel；`part_05.rs:345-353` 明确默认 direct-message lease 是 system-owned，仅显式 owner unsubscribe 才持久停止，其余 cancelled 都会被 re-arm。（此条置信度中；可能“auto-notify”指 keepalive，需 owner 定夺。）
- 影响：下一次 `install-global-collab.sh` 会把这两处发布给所有 agent；第一条会让 operator 的 reset 命令直接失败。
- 最小修订：`migration-daemon.md:55,140` 恢复 `--project`；`task-worktree-lifecycle.md:59-60` 与 `part_09.rs`/`part_05.rs` 的实际语义对齐。

#### 证据 I｜已安装 skill bundle 落后于仓库源（安装状态漂移，非源码 bug）

- `diff /Users/fanzhang/.agents/skills/collab/SKILL.md collab/skills/collab/SKILL.md`：仅 frontmatter `description` 一行不同（仓库版更丰富）；`references/migration-daemon.md`、`references/task-worktree-lifecycle.md` 亦不同（即证据 H 的两处）。
- 已安装二进制 `collab 0.2.0258`（`/Users/fanzhang/.cargo/bin/collab`）；全局 skill mtime 2026-10-08 19:27，仓库 2026-10-09。
- 影响：发布/升级前必须重跑 `collab/scripts/install-global-collab.sh`（含 build counter、`cmp` 7 文件、sha256 校验），否则规则/skill 真源与已安装态继续分叉。

### 节点 3｜DAG 重建（静态）

- 入口层：CLI `src/main.rs`（`Cli`/`Cmd`，`main_cli.rs` 定义各 `*Cmd`）与 MCP `src/bin/collab-mcp.rs`（工具名 → argv → spawn 同目录 `collab`）。两者都经 `client::call_with_context` 把 `RequestEnvelope{project_context, Req}` 写 Unix socket。
- 传输/路由层：host daemon `conn_task_routed`（`part_12.rs:259`）→ `dispatch_wire_routed`（`part_12.rs:103`）→ 路由类（RouteResolve*）或 `ProjectRuntimeManager::dispatch_sync`（`part_04.rs:1078`）。`dispatch_sync` 先做 `validate_current_thread_candidate`、`CrossProjectSend` 特判、`validate_request_context`（`part_11.rs:945`），再进 `dispatch_with_route_context`（`part_11.rs:1`）。`Req::Poll` 走独立 async 路径（`part_12.rs:148`）。
- owner 层（各自唯一 owner）：身份 = `GlobalState`/`RuntimeBinding` + `identity_resolver`；消息 = `State.msgs` + `mailbox.rs` 投影；任务 = `State.tasks` + `TaskRec`；资源/工作树 = `WorktreeBinding` + `config.worktree`；授权 = `GlobalMasterGrant`（typed）；通知 = `NotificationSubscription` + `WakeAttempted`；调度准入 = `SchedulerAdmissionRecord`。
- reducer/journal 层：`Server.commit_locked`/`commit_locked_checked`/`commit_command_locked`（`part_02.rs`）先 append+fsync journal，再 `apply_committed_events`→`State::apply_checked`；reducer 拒绝则回滚 journal 长度并 poison（`part_02.rs:924-943`）。启动 `replay_from_journal`（`part_12.rs:370`）重建 `State` 与 `global`。
- 公开投影层：`board_show`（只读，authority 与 transport observation 分开，`board_handlers.rs:49`）、`context`（`part_09.rs:1051`）、`worker status`、`dashboard`（只读 loopback + capability，`dashboard.rs`）、`mailbox_read` recipient JSONL 投影（`part_11.rs:673`）。
- 两条真源：host-wide typed state（`~/.collab`/`GlobalState`/`GlobalEvent`）与 project runtime state（`.agent-collab/server/journal.jsonl`/`State`/`Event`）；`routes.jsonl` 是路由投影。

### 节点 4｜覆盖收口

- 状态：完成（静态只读）
- 已读覆盖：`collab/` 全部生产源码按模块分组读过/结构核对过（入口、proto、server mod+mod_parts 全部、state/global_state、board/dashboard、mailbox/notification/keepalive/timers、identity/identity_resolver、subagent、scope/config/client/reset/migration、adapters、MCP）；Cargo/build.rs；`scripts/*`；`.github/workflows/verify.yml`；`collab/skills/collab/**`；`collab/docs/**` 关键文件与 `docs/dagpipe/collab-*.graph.json`。
- 未验证边界：任何运行期行为（未跑 test/build/install/daemon）；`~/.collab`/`.agent-collab` 实时状态；tmux/AppServer/dsh 真实端点；`docs/evidence/**` 历史日志只作线索，未复核其为当前基线证据。
- 未运行：任何测试、build、安装、daemon 生命周期命令。
