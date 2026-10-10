# foundations auditor notes - rule-upgrade-audit-20261009

## 任务锚定

- 任务：AppSDK 规则升级与全架构审计并行采证；本 worker 负责 foundations 只读范围。
- 基线：`3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`；cwd `/Users/fanzhang/Documents/github/appsdk`。
- 只读责任：`rust/src/communication*`、`rust/src/global_registry_communication.rs`、`rust/src/memory*`、`rust/src/bin/**`、`rust/src/dagpipe*`、`rust/src/long_horizon*`、整个 `dagpipe/`、bench 当前工具代码；其他范围归 core/runtime/rules auditor。
- 只写本目录 `notes.md` 及原始观察；产品源码只读。
- 禁止跑测试/build/安装/重启/commit/push，禁止派 agent、改长期记忆/全局规则。

## 范围盘点

- `rust/src/communication.rs` 1059 行；`rust/src/communication/` 8 文件共约 7713 行（helpers 888, store_core 1118, store_delivery 1414, store_events 890, store_journal 788, store_messaging 1022, store_runtime 955, validation 638）。
- `rust/src/global_registry_communication.rs` 584 行；作为 `global_registry.rs:36-38` 的 `mod communication` 挂载并 re-export。
- `rust/src/memory.rs` 1494 行 + `memory_cli.rs` 913 行；`rust/src/bin/project-memory.rs` 6 行调用 `memory::main()`。
- `rust/src/dagpipe.rs` 1084、`dagpipe_notification.rs` 953、`dagpipe_tests.rs` 204；`rust/Cargo.toml` 依赖 `pipeline_runtime = { path = "../dagpipe" }`。
- `rust/src/long_horizon_role.rs` 123、`long_horizon_policy.rs` 224。
- `dagpipe/` 独立 package：`Cargo.toml`、`src/lib.rs` 3159、`src/bin/dagpipe.rs` 176、4 个 examples、`scripts/install.sh` 129、README/docs/Skill。
- bench：只有 `bench/results/team_*.json` 4 份历史结果，无现行工具源码；作为“无 code”覆盖项，不读海量历史日志。
- 子模块挂载方式：`communication.rs:934-951` 用 `#[path]` 内嵌 8 个文件；`dagpipe.rs:297,1083` 内嵌 notification 与 tests。

## 已确认公开入口与 owner

- `appsdk communication|comm <project> --json '<request>'`：`main.rs:1486-1489` -> `communication::run_cli`；`communication.rs:954-1012`。
- `appsdk communication capabilities`：只输出静态 capability，不打开 store（`communication.rs:955-958`）。
- `appsdk communication reset-runtime-registry --discard-legacy --approval <text>`：`communication.rs:959-989` -> `global_registry::reset_runtime_registry`。
- `appsdk dagpipe ...`：`main.rs:1491-1492` -> `dagpipe::run_cli`；用法 `fix|validate|validate-notifications`（`dagpipe.rs:32-60`）。
- `project-memory`：`main.rs:1101` -> `memory::run`，入口 `rust/src/bin/project-memory.rs`。
- `dagpipe` 独立 binary：`dagpipe/src/bin/dagpipe.rs`，公开图命令 `validate|inspect|sdk-path|install-skill|(...)`。
- 通信持久化：项目事实源 `.appsdk-control/communication/mailbox.jsonl`；host 运行时 `~/.appsdk/runtimes.jsonl`；host 发现索引 `~/.appsdk/communication.jsonl`。
- 设计契约：`docs/design/apps-sdk-communication.md`、`docs/design/appsdk-global-registry.md`、`docs/design/project-memory.md`、`docs/design/dagpipe-internal-module.md`。

## 节点状态

- 2026-10-10 | 通信入口/owner/持久化/global discovery | 完成 | communication.rs + global_registry_communication.rs + contracts/directories | 基线 HEAD | 下一步：store_journal/store_delivery replay 兼容与失败隔离 | 已发现需要继续核对的观察：`replay_identity_only` 对 cross-project 只重放 identity 事件；完整 mailbox 由目标 owner 打开时校验。
- 2026-10-10 | 通信 store 结构与失败路径 | 完成 | store_core/store_runtime/store_messaging/store_journal/store_delivery/store_events/validation/helpers | 基线 HEAD | 下一步：核对 schema 与事件处理一致性、测试清单 | 记录：`run_cli` 在 dispatch error 后 `record_error_event`，若二级写入失败返回双重错误链；`open` 会在项目根不存在时 `create_dir_all`（设计文档仅称 root 必须存在；代码会创建 root，见 store_core.rs:6-18）。
- 2026-10-10 | 通信 schema/事件处理一致性 + 测试清单 | 完成 | contracts/communication/*.schema.json + rust/tests/communication_* | 基线 HEAD | 下一步：memory 范围 | 事实：`scope.unregistered` 仅在 replay 处理（store_events.rs:23、store_journal.rs:168、global_registry_communication.rs:184、event schema enum），全仓无 writer/CLI op；设计文档未描述 scope 注销。测试为真实集成测试（rust/tests/communication_cli/main.rs 用 `CARGO_BIN_EXE_appsdk` 起真实 binary；7 文件 99 个 #[test] + event/request schema 25 个）。`open` 缺根创建无任何调用方依赖（唯一调用点 communication.rs:1001 直接传 CLI 参数）。
- 2026-10-10 | memory 范围审计 | 完成 | memory.rs + memory_cli.rs + bin/project-memory.rs + docs/design/project-memory.md + guidance/projector.rs + cli_smoke/part_14.rs | 基线 HEAD | 下一步：dagpipe | 事实：JSONL 为唯一真源，SQLite/Markdown 为可重建投影；模块自述不感知 Guide/生命周期，`review` 是唯一 run-note 回写桥；projector.rs:274-276 `memory_applied:false`、`memory_review.blocking:false` → memory 非控制真源。观察：index_profile 写 `schema_version=2`（memory.rs:1475）但 `verify` 报 1（memory_cli.rs:591）且 `MEMORY_SCHEMA_VERSION=1`，verify 从不读回该键；`query` 宣称 node/function/resource 组（memory_cli.rs:387 `node_matches:[]` 硬编码空、906 help、index_profile query_order）但无 node 匹配实现。
- 2026-10-10 | dagpipe 应用模块审计 | 完成 | rust/src/dagpipe.rs + dagpipe_notification.rs + dagpipe_tests.rs + docs/design/dagpipe-internal-module.md + cli_smoke/part_17,part_19 | 基线 HEAD | 下一步：dagpipe 独立包 | 事实：fix-lifecycle/notification 两个可执行图由 FixOperator/NotificationObjectValidateOperator 做真实跨记录业务校验（引用一致性），输出自述 `authority:advisory_projection`、`authoritative_gate:appsdk verify`；13 张 design 图用 DesignGraphOperator 桩（execute 恒 `DAGPIPE_DESIGN_OPERATOR_NOT_EXECUTABLE`），仅做 SESE 拓扑+compile，不可执行。拓扑唯一 owner 为 `pipeline_runtime::graph_topology`，app 层 `ensure_single_source_single_sink` 只做本地形状检查再委托，无第二实现。manifest(docs/dagpipe/manifest.json) 与 embedded_graph_paths() 15 项交叉校验，属有意双源守卫。
- 2026-10-10 | dagpipe 独立包审计 | 完成 | dagpipe/Cargo.toml + src/lib.rs + src/bin/dagpipe.rs + examples + scripts/install.sh + README/docs + .agents skill | 基线 HEAD | 下一步：long_horizon | 事实：`pipeline_runtime` 0.1.1，唯一 bin `dagpipe`；rust/Cargo.toml:16 用 path 依赖 `../dagpipe`；`graph_topology` 强制 SESE（1 输入/1 输出、无环、无死节点、单 sink）；`compile` 是完整注册/类型/能力门禁；CLI `graph validate|inspect` 只做形状证据、自述 project compile 才是权威。lib.rs 28 个内联 #[test]；`scripts/install-global-dagpipe.sh`(root) exec `dagpipe/scripts/install.sh`，两者都存在，docs 引用正确，非缺陷。install.sh 有 symlink/归属/清单守卫与原子替换。
- 2026-10-10 | long_horizon 范围审计 | 完成 | long_horizon_policy.rs + long_horizon_role.rs | 基线 HEAD | 下一步：汇总报告 | 事实：policy 为纯提示词文本（无预算字段，测试断言不含 token_budget）；role 解析只从已验证 `collab context`+live worker/master 记录得出，任何 probe 不可用→Unknown，无 master fallback；仅消费 Collab（`collab context`/`collab_master_status`），不复制 Collab 状态 owner。main/goal.rs、main/longhorizon.rs 属 core/main 范围，未纳入本 worker 审计。
- 2026-10-10 | bench 覆盖确认 | 完成 | bench/results/*.json | 基线 HEAD | 下一步：报告 | 事实：bench 仅 4 份历史结果 JSON，无现行工具源码；按指令不读海量历史日志，作为“无 code”覆盖项。
