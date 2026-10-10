Audit complete. Below is the report for `report.md`.

---

# collab/ 架构与规则审计报告

任务 `rule-upgrade-audit-20261009` / worker `appsdk-audit-runtime`；基线 `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`；全程只读，未运行测试/build/安装/daemon 命令，未查看或改变 `~/.collab`、`.agent-collab` 状态。**无测试/产品 PASS 声明。**

## 1. 覆盖

已读/结构核对：`collab/` 全部生产源码（入口 `main.rs`/`main_cli.rs`/`main_context.rs`/`main_live_closure.rs`；`proto.rs`；`server/mod.rs` 与 `mod_parts/part_01..13`、`board_handlers/board_invites/board_execution`、`state.rs`、`global_state*`、`mailbox.rs`、`notification_contract.rs`、`notification_state.rs`、`keepalive.rs`、`timers.rs`、`identity_context.rs`；`identity.rs`、`identity_resolver.rs`、`subagent.rs`、`scope.rs`、`config.rs`、`client.rs`、`reset.rs`、`migration.rs`/`migration_tail.rs`、`adapters/**`、`bin/collab-mcp.rs`、`install_skills.rs`）；`Cargo.toml`/`Cargo.lock`/`build.rs`；`scripts/*`；`.github/workflows/verify.yml`；`collab/skills/collab/**`；`collab/docs/**` 关键文件与 `docs/dagpipe/collab-*.graph.json`。

未验证边界（未获得运行期证据）：任何 test/build/install/daemon 行为；`~/.collab`/`.agent-collab` 实时状态；tmux/AppServer/dsh 真实端点；`docs/evidence/**` 历史日志仅作线索，未复核为当前基线证据。

## 2. DAG（静态重建）

- 入口：CLI [main.rs](/Users/fanzhang/Documents/github/appsdk/collab/src/main.rs:349) 与 MCP [collab-mcp.rs](/Users/fanzhang/Documents/github/appsdk/collab/src/bin/collab-mcp.rs:178) 都把 `RequestEnvelope{project_context, Req}` 经 Unix socket 发到 host daemon。
- 路由：`conn_task_routed` [part_12.rs:259](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_12.rs:259) → `dispatch_wire_routed` [part_12.rs:103](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_12.rs:103) → `ProjectRuntimeManager::dispatch_sync` [part_04.rs:1078](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_04.rs:1078) → `validate_request_context` [part_11.rs:945](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_11.rs:945) → `dispatch_with_route_context` [part_11.rs:1](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_11.rs:1)；`Poll` 走独立 async 路径。
- owner：身份=`GlobalState`/`RuntimeBinding`+`identity_resolver`；消息=`State.msgs`+`mailbox.rs` 投影；任务=`State.tasks`；资源=`WorktreeBinding`+`config.worktree`；授权=`GlobalMasterGrant`；通知=`NotificationSubscription`+`WakeAttempted`；调度准入=`SchedulerAdmissionRecord`。
- reducer/journal：`commit_locked*`（[part_02.rs](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_02.rs:863)）先 append+fsync 再 `apply_committed_events`；reducer 拒绝则回滚 journal 长度并 poison（part_02.rs:924-943）；启动 `replay_from_journal` [part_12.rs:370](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_12.rs:370)。
- 公开投影：`board show`（只读，authority 与 transport observation 分离，[board_handlers.rs:49](/Users/fanzhang/Documents/github/appsdk/collab/src/server/board_handlers.rs:49)）、`context` [part_09.rs:1051](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_09.rs:1051)、`dashboard`（只读 loopback+capability，[dashboard.rs](/Users/fanzhang/Documents/github/appsdk/collab/src/dashboard.rs:52)）、recipient JSONL 投影 [part_11.rs:673](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_11.rs:673)。
- 两条真源：host-wide typed state（`~/.collab`/`GlobalState`/`GlobalEvent`）与 project runtime state（`.agent-collab/server/journal.jsonl`/`State`/`Event`）；`routes.jsonl` 是路由投影。

## 3. 确定 finding

**F1（高）`collab subagent dispatch` 及 ready/working/send/snapshot/rearm/close 在 CLI/MCP 静默 no-op，daemon 调度/受管子代理路径不可达。**
- 位置：[main.rs:356](/Users/fanzhang/Documents/github/appsdk/collab/src/main.rs:356) `Cmd::Subagent` 仅 `Start` bail、`List/Status` 走 `SubagentObserve`，其余落 `Ok(())`；[main_context.rs:358](/Users/fanzhang/Documents/github/appsdk/collab/src/main_context.rs:358) `subagent_observe_query` 只认 List/Status。
- 消费者依据：SKILL 主路径 [SKILL.md:547](/Users/fanzhang/Documents/github/appsdk/collab/skills/collab/SKILL.md:547)、模板 `:659`、`ready|working` `:529`；MCP schema/描述 [collab-mcp.rs:28](/Users/fanzhang/Documents/github/appsdk/collab/src/bin/collab-mcp.rs:28)、argv 映射 `:217`。daemon 侧实现完整：[part_11.rs:41](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_11.rs:41) → [subagent.rs:812](/Users/fanzhang/Documents/github/appsdk/collab/src/subagent.rs:812)、`handle_scheduler_dispatch` [part_07.rs:853](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_07.rs:853)。生产代码无任何构造 `Req::Subagent` 的点（仅 tests）。
- 影响：文档化的 master 派单与子代理生命周期在 CLI/MCP 不可达；CLI exit 0 无输出、MCP `isError:false` 空文本（[collab-mcp.rs:178](/Users/fanzhang/Documents/github/appsdk/collab/src/bin/collab-mcp.rs:178)），违反“错误显式、不伪造成功”。
- 最小修订：非 `Start` action 经 `Req::Subagent{worker_id,token,command,launch_env:BTreeMap::new()}` 送 daemon；若确不支持则显式 fail。
- 定向验证：`rg -n "subagent_observe_query|Cmd::Subagent|Req::Subagent" collab/src/main.rs collab/src/main_context.rs collab/src/server/mod_parts/part_11.rs`；黑盒（需授权）`collab subagent dispatch --request-id x --subject s "body"` 应产生 task/admission 而非无输出 exit 0。

**F2（高，发布阻断）`migration-daemon.md` 的 reset 命令缺必填 level flag，会直接失败。**
- 位置：[migration-daemon.md:55](/Users/fanzhang/Documents/github/appsdk/collab/skills/collab/references/migration-daemon.md:55)、`:140` 写 `collab reset --discard-legacy`。
- 消费者依据：CLI 要求 `--project|--host` [main_cli.rs:355](/Users/fanzhang/Documents/github/appsdk/collab/src/main_cli.rs:355)，`ResetLevel::select` 二者皆缺即 bail [reset.rs:44](/Users/fanzhang/Documents/github/appsdk/collab/src/reset.rs:44)，`main.rs:770` 直接调用；同仓库 `state-paths.md:40`、`SKILL.md:247-248` 仍写 `--project/--host`，故本文件为错处。
- 影响：下次 `install-global-collab.sh` 会发布这条必失败命令。
- 最小修订：恢复 `collab reset --project --discard-legacy`。验证：`rg -n "collab reset" collab/skills/collab/references/migration-daemon.md`。

**F3（中）三份 canonical 文档仍描述旧的 liveness-gated master authority，与代码及 dagpipe 图矛盾。**
- 位置：[resource-map.md:6](/Users/fanzhang/Documents/github/appsdk/collab/docs/resource-map.md:6)、[function-map.md:6](/Users/fanzhang/Documents/github/appsdk/collab/docs/function-map.md:6)、[verification-map.md:6](/Users/fanzhang/Documents/github/appsdk/collab/docs/verification-map.md:6) 仍写 “tmux pane liveness probe / native RPC presence is explicitly absent”。
- 消费者依据：代码明确 liveness 不参与授权 [part_07.rs:1193](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_07.rs:1193)、`:1292`、`:831`；[board_handlers.rs:70](/Users/fanzhang/Documents/github/appsdk/collab/src/server/board_handlers.rs:70) 分离 authority 与 observation；`docs/dagpipe/collab-master-authority.graph.json` 已描述新语义。
- 影响：规则升级前真源自相矛盾。最小修订：三行改为“typed grant 授权，transport liveness 独立观察”。

**F4（中）CI 完全不含 `collab/` gate。**
- 位置：[verify.yml](/Users/fanzhang/Documents/github/appsdk/.github/workflows/verify.yml:1) 三个 job 仅覆盖 `rust/` 与 `dagpipe/`；`collab/` 是独立 Cargo 包（[Cargo.toml](/Users/fanzhang/Documents/github/appsdk/collab/Cargo.toml:1)+自带 lock），`rg -rn collab .github/` 无命中；gate 只以手写命令散落在 docs。
- 影响：F1 这类入口回归 CI 无法捕获。最小修订：加 `collab` fmt --check 与 `cargo test --manifest-path collab/Cargo.toml --bin collab --bin collab-mcp`。

**F5（中）`client::adapters` 的 `legacy` 子模块约 800–1000 行无消费者（删除候选）。**
- 位置：[client.rs:1](/Users/fanzhang/Documents/github/appsdk/collab/src/client.rs:1) 载入 [adapters/mod.rs:20](/Users/fanzhang/Documents/github/appsdk/collab/src/adapters/mod.rs:20) `mod legacy`。
- 消费者依据：`AdapterRegistry`/`AppServerAdapter`/`submit_registered`/`interrupt_registered`/`UnavailableAdapter`/`EndpointBinding`/`binding_for_*`/`validate_*` 的 `rg -l` 只命中自身（tests 亦在文件内）；仅 `EndpointKind/WakeMode/AdapterCapabilities/AdapterError/NotificationDeliveryClass` 被 `codex_app_server_production_part1.rs`、`part_05.rs` 使用。编译器同证 `docs/evidence/collab-master-authority-fix-20261007/candidate-install.log:452-559`。
- 最小修订（需 owner 确认）：只保留被消费的 5 个类型。验证：`cargo build --manifest-path collab/Cargo.toml 2>&1 | rg "never used"` 应无相关命中。

**F6（中）`migration.rs`+`migration_tail.rs`（约 2850 行）无生产消费者（删除/接线候选）。**
- 位置：[main.rs:7](/Users/fanzhang/Documents/github/appsdk/collab/src/main.rs:7)；`migration.rs:77-80`。
- 消费者依据：`rg -n "\bmigration::" collab/src collab/tests` 无命中；真实迁移功能由 `state.rs` `MigrationRecord`+[part_06.rs:503](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_06.rs:503) 实现。编译器同证 `candidate-install.log:634-899` 整片 never used。
- 最小修订：接到真实 `collab migrate` 入口，或删除/移出编译。

**F7（低）三张 dagpipe 图描述与实现不符。**
- `docs/dagpipe/collab-context.graph.json`、`collab-dashboard.graph.json`、`collab-master-authority.graph.json` 描述结尾均写“实现代码边当前待实现”，但 handler 已存在：[part_09.rs:1051](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_09.rs:1051)、[board_handlers.rs:49](/Users/fanzhang/Documents/github/appsdk/collab/src/server/board_handlers.rs:49)、[part_07.rs:1176](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_07.rs:1176)，并在 [part_11.rs:29](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_11.rs:29) 挂载。最小修订：移除该句或改指真实 handler。

## 4. 假设（未定论，勿当根因）

- H1：`task-worktree-lifecycle.md:59-60`（仓库版）称“Last owned close cancels this peer's direct-message auto-notify”，但 `handle_task_close` [part_09.rs:325](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_09.rs:325) 只 supersede keepalive、清计数，且默认 direct-message lease 是 system-owned、仅显式 unsubscribe 才停 [part_05.rs:345](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_05.rs:345)。可能“auto-notify”指 keepalive，需 owner 定夺。
- H2：已安装 skill bundle 落后仓库（`~/.agents/skills/collab/SKILL.md` 与仓库仅 frontmatter description 不同，两个 reference 文件不同；已安装二进制 `0.2.0258`）。属发布状态而非源码缺陷，发布前须重跑 `install-global-collab.sh`。
- H3：`docs/mainline-call-map.json` 的 `path:"src/server/mod.rs"` 指向 `include!` 进来的 `mod_parts/*`，语义上成立，不算漂移。

## 5. 删/复用建议

- 复用（勿新写）：daemon 的 `handle_scheduler_dispatch`、`subagent::run`、`handle_master_promote/clear/delegate`、`handle_context`、`handle_board_*` 均已实现且被挂载，F1 只需接线 CLI/MCP。
- 删除候选（需 owner 确认）：`adapters::legacy` 未消费面（F5）；`migration.rs`/`migration_tail.rs` 未接线面（F6）。
- 不删：旧 wire 变体 `MasterRecover/TransferMaster/RemoveWorker/ResetBindings/TaskDispatch/TaskClaim` 是显式 fail-closed 兼容（[part_11.rs:569](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_11.rs:569)、[part_08.rs:1015](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_08.rs:1015)），无生产者≠死代码。

## 6. 保留理由

- 两条真源（host `GlobalState` + project `State`）与 journal 先 fsync 后 apply、reducer 拒绝回滚并 poison，是持久化保障，保留。
- `CrossProjectSend` 在 routed 路径可用（[part_04.rs:1134](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_04.rs:1134) 早于 [part_11.rs:138](/Users/fanzhang/Documents/github/appsdk/collab/src/server/mod_parts/part_11.rs:138) 的拒绝），`collab master send` 不是坏链。
- dashboard 只读、loopback、capability、CSP、Host 校验齐全（[dashboard.rs](/Users/fanzhang/Documents/github/appsdk/collab/src/dashboard.rs:32)），保留。

## 7. 需要真实验证的风险（UNVERIFIED）

- F1 的黑盒行为（CLI/MCP 真 no-op）与 daemon `handle_scheduler_dispatch` 真可达性。
- 已安装二进制 `0.2.0258` 与基线源码是否已有行为差异。
- tmux/AppServer/dsh 真实端点与 `routes.jsonl` 实时投影。
- F6 migration 模块是否被设计为“later adapter”（需 owner 确认，非自动删）。
- 任何 test/build/install 结论：本轮全部未运行，不得据此声称 PASS。
