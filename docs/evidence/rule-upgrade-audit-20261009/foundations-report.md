# report.md — foundations 审计（worker: appsdk-audit-foundations）

基线 `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`，只读，无 build/test/安装/重启/commit/push。以下为静态审计；不含任何测试或 runtime PASS。

## 覆盖（已读清单）

- 通信：`rust/src/communication.rs`(1059) 及 `#[path]` 8 子模块（helpers/store_core/store_delivery/store_events/store_journal/store_messaging/store_runtime/validation，共约 7713 行）、`rust/src/global_registry_communication.rs`(584)。
- 记忆：`rust/src/memory.rs`(1494)、`rust/src/memory_cli.rs`(913)、`rust/src/bin/project-memory.rs`。
- DAGPipe 应用：`rust/src/dagpipe.rs`(1084)、`dagpipe_notification.rs`(953)、`dagpipe_tests.rs`(204)。
- DAGPipe 独立包：`dagpipe/Cargo.toml`、`src/lib.rs`(3159)、`src/bin/dagpipe.rs`、4 examples、`scripts/install.sh`、`README.md`、`docs/**`、`.agents/skills/dagpipe-runtime/SKILL.md`。
- 长程：`rust/src/long_horizon_policy.rs`(224)、`rust/src/long_horizon_role.rs`(123)。
- 契约/设计：`docs/design/{apps-sdk-communication,appsdk-global-registry,project-memory,dagpipe-internal-module}.md`、`contracts/communication/*.schema.json`、`contracts/maps/module-registry.json`、`docs/dagpipe/manifest.json`。
- 测试来源：`rust/tests/communication_cli/**`(7 文件 99 `#[test]`)、`communication_event_schema.rs`(5)、`communication_request_schema.rs`(20)、`cli_smoke/part_14,17,19.rs`。
- bench：仅 `bench/results/team_*.json` 4 份历史结果，无现行工具源码（按指令不读海量日志）。

## 公开入口 → 唯一 owner → 状态/执行/结果 DAG

- `appsdk communication|comm <project> --json`：`main.rs:1486-1489` → `communication::run_cli`（`communication.rs:954-1012`）→ `CommunicationStore::open`（`store_core.rs:6`）。事实源 `.appsdk-control/communication/mailbox.jsonl`；host 运行时 `~/.appsdk/runtimes.jsonl`；host 发现索引 `~/.appsdk/communication.jsonl`。`capabilities` 静态、不开 store。
- `project-memory`/`appsdk memory`：`main.rs:1101` 与 `bin/project-memory.rs` → `memory::run`（`memory_cli.rs:695`）。真源 `memory/{plan,path,knowledge,lesson}.jsonl`；SQLite/Markdown 为可重建投影。
- `appsdk dagpipe fix|validate|validate-notifications`：`main.rs:1491-1492` → `dagpipe::run_cli`（`dagpipe.rs:28`）。拓扑唯一 owner `pipeline_runtime::graph_topology`。
- `dagpipe` 独立 binary：`dagpipe/src/bin/dagpipe.rs`；`rust/Cargo.toml:16` 以 `path = "../dagpipe"` 复用同一 `pipeline_runtime`。
- `appsdk longhorizon`：policy 生成提示词，role 由已验证 Collab 上下文解析。

## 已确认事实 finding

**F1（通信 root 创建越界）** `store_core.rs:6-18` 在 root 不存在时 `create_dir_all(root)`，与设计契约「命令 root 必须是存在的绝对 canonical 项目路径」（`apps-sdk-communication.md:39`）冲突。消费者：唯一调用点 `communication.rs:1001` 直接把 CLI 参数当 root；`delivery_retry.rs:127,137` 只测非 canonical 拒绝，无测缺根创建。后果：对不存在的 canonical 路径执行读/查询会凭空建项目目录与 mailbox。最小修订：`open` 拒绝缺失 root，保留 `open_mailbox_at` 对 mailbox 父目录的创建（`store_core.rs:57-70`）以保证正常流程。保留保障：canonical/symlink/非目录校验与 mailbox 父目录创建不动。无调用方依赖缺根创建。

**F2（scope.unregistered 无 writer）** 事件仅在 replay 分支处理：`store_events.rs:23`、`store_journal.rs:168`、`global_registry_communication.rs:184`，并列入 `communication-event.schema.json:13,100`；全仓无 append/CLI op，设计文档亦未描述 scope 注销（仅 agent.rebound tombstone）。后果：一段无写入者的投影分支与 schema 枚举，属前向兼容/死路径。最小修订：在 schema/设计注明为 reserved，或删除 replay 分支+枚举（当前无历史数据含该 kind）。保留保障：`scope.registered`、`agent.rebound` tombstone 语义不动。

**F3（memory 索引 schema 版本非单一真源）** `memory.rs:1475` 写 `index_profile.schema_version='2'`，但 `verify` 硬编码输出 `schema_version:1`（`memory_cli.rs:591`），`MEMORY_SCHEMA_VERSION=1`（`memory.rs:18`），migration `target_schema=1`（`memory.rs:855,895,922`）；`verify` 从不读回该键，故版本漂移不可检测。后果：三处版本号互相矛盾。最小修订：让 `verify` 读回 `index_profile.schema_version` 并与常量一致，或统一为 1。保留保障：source_digest/nodes/fts 一致性检查不动。定向测试：`cli_smoke/part_14.rs` 覆盖 index/verify/source-drift。

**F4（memory 宣称的 node/function/resource 组未实现）** `query` 输出 `node_matches: []` 硬编码（`memory_cli.rs:387`），help 与 index_profile 的 `query_order` 仍列 “node/function/resource”（`memory_cli.rs:906`、`memory.rs:1482`），无 node 匹配逻辑。后果：按文档顺序检索的 agent 拿不到该组结果。最小修订：实现最小 node 匹配，或删除该组声明与空字段。保留保障：exact/category/declared/lesson/fts 组与 `detail_path` 不变。

## 静态假设 vs 已确认事实

- 事实（代码/契约直接可证）：F1–F4；memory 非控制真源（`guidance/projector.rs:269` `memory_applied:false`、`memory_review.blocking:false`，模块头注释自述不感知 Guide/生命周期）；DAGPipe 设计图用 `DesignGraphOperator` 桩（`dagpipe.rs:482` 恒 `DAGPIPE_DESIGN_OPERATOR_NOT_EXECUTABLE`）仅做拓扑+compile；fix/notification 图输出自述 `authority:advisory_projection`、`authoritative_gate:appsdk verify`（`dagpipe.rs:148-149`）；拓扑唯一 owner，无第二实现（`ensure_single_source_single_sink` 只做本地形状检查再委托 `graph_topology`）；long_horizon 无 master fallback（`long_horizon_role.rs:11-16`）。
- 静态假设（需运行/外部核对，未证实）：F1 是否真的会被外部调用者以缺根路径触发；F2 是否有外部生产者在别处写 `scope.unregistered`；F3/F4 是否有下游消费方依赖 `schema_version` 或 `node_matches`。

## 最小删/复用建议（不凭名字删）

- 复用：DAGPipe 通用图引擎唯一源码在 `dagpipe/`，`rust/` 仅 path 依赖；SESE 拓扑唯一 owner `graph_topology`，不要在各调用点复制校验。
- 可删（有调用/保留判据后）：F2 的 replay 分支+schema 枚举（无 writer、无历史数据）；F4 的空 `node_matches` 字段与 “node/function/resource” 声明（无实现）。
- 不删：13 张 design 图的 `DesignGraphOperator` 桩（拓扑/合同证据用，故意不可执行）；`embedded_graph_paths()` 与 `manifest.json` 双源（`validate_graph_contracts_with_manifest` 交叉校验，是有意守卫，非重复）；`scripts/install-global-dagpipe.sh`↔`dagpipe/scripts/install.sh`（前者薄封装后者，均存在，非缺陷）。

## 保留理由（安全/契约/持久化完整性）

- 通信：JSONL 为唯一事实源、投影不可写；`replay_identity_only` 跨项目只重放 identity 以免锁环（`store_core.rs:25-55`）；`record_delivery` 单调推进、legacy receipt 只读重放不放宽校验；adapter 失败保留原错并记 `notification.delivery_failed`，不伪造 delivered。
- 记忆：JSONL append-only，SQLite 可删可重建；`review` 是唯一 run-note 回写桥；`migrate`/`import` 显式、源保留、冲突显式失败；symlink 全面拒绝。
- DAGPipe：`compile` 是完整注册/类型/能力门禁，`CompiledGraph` 冻结拓扑；CLI 只做形状证据并自述权威在项目 compile。

## 定向验证（建议，未执行）

- F1：`appsdk communication <不存在的canonical路径> --json '{"op":"status"}'` 观察是否创建目录；`cargo test` 中补缺根拒绝红测。
- F2：全仓 `rg` writer 已做；如需确认外部生产者，查 Collab/core 范围（引用，不重审）。
- F3/F4：`project-memory verify` 与 `query` 输出核对 `schema_version`/`node_matches`；`cli_smoke/part_14.rs` 已有相关断言可扩。
- 已有入口：`rust/tests/communication_cli/**`、`communication_{event,request}_schema.rs`、`cli_smoke/part_14,17,19.rs`、`dagpipe/src/lib.rs` 内联 28 测试。

## 未知 / 未验证

- 未运行任何测试/build，未验证 F1 触发路径与 F3/F4 下游依赖。
- `main/goal.rs`、`main/longhorizon.rs`、core、Collab、规则范围属其他 auditor；本报告仅引用，不重审。
- 跨范围精确问题：`long_horizon_role.rs` 消费 `collab context`/`collab_master_status`，其字段契约 owner 在 Collab 范围，需 Collab auditor 确认字段稳定性（本 worker 不重审）。
