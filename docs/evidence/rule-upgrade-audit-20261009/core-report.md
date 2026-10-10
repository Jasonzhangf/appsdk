I've completed the read-only audit. Here is the deliverable report for `report.md`.

---

# AppSDK Core SDK 架构审计报告

- 任务：`rule-upgrade-audit-20261009`
- Worker：`appsdk-audit-core`
- 基线：`3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`（与 `origin/main` 一致，工作树初始干净）
- 模式：只读静态审计。未运行测试、构建、安装、重启、提交或推送；未派 agent；未注册 Collab。
- 版本事实：`rust/release-version = 0.1.0012`，`SDK_VERSION` 由 `rust/build.rs:19` 注入；当前安装二进制版本未验证（运行时 auditor 负责）。

## 1. 覆盖与被读文件清单

已完整读取：`rust/src/main.rs`、`main/{cli,init,migration,reset_governance,project,governance,registry,requirements,canonical_map}.rs`、`guidance.rs` 及 `guidance/{intake,compiler,ledger,projector,review,help}.rs`、`global_registry.rs`、`global_registry_communication.rs`、`global_registry_tests.rs`、`rust/tests/sdk_0008_migration.rs`、`sdk_0009_migration.rs`、`rust/tests/cli_smoke/{part_01..part_24}.rs` 中与 init/pin-lock/migration/guidance/requirements/communication 相关用例、`docs/design/appsdk-guidance-framework.md`、`docs/architecture/development-process-control-harness.md`。

已扫描（骨架 + 入口 + 关键分支）：`main/{compile,promotion,review_gates,verification,producer,producer_commit,merge_gates,lifecycle_chain,lifecycle_closure,rehydrate,reset_run,reset_transaction,reset_validate,review_context,longhorizon,goal,bug_cli,bug_tracker,test_governance}.rs`。

未做全行精读的边界：`compile.rs`/`promotion.rs`/`verification.rs` 内部逐分支语义、`communication/store_core.rs` 消费侧（不在本 worker 范围）、嵌入式 bundle 内容字节级校验、安装后二进制行为。这些不冒充全读。

## 2. 真实 CLI → owner → 合同/状态 → 公开结果 DAG

- 入口分派：`rust/src/main.rs:1081-1496`（`version/verify/requirements/review-context/guide/memory/bug/setup-deps/goal/longhorizon/task/pin-lock/sdk-witness/reset-governance/compile*/promote*/freeze/publish-active/new/init/prepare/communication/dagpipe`）。
- 新项目：`new` → `new_project`（`main/migration.rs:12-57`）→ `ensure_governance_layout`+`write_project_scaffold`+`install_bundle_resources`+`write_current_sdk_lock`+`install_standard_template_reference`。
- 既有项目：`init` → `existing_init_target` → `init_project(root,false,false)`（`main.rs:1453-1455`）→ 同一 owner 链（`main/init.rs:842-898`）。
- 治理 bundle owner：`install_bundle_resources`（`main/governance.rs:247-296`）写 `.appsdk/contracts/**`、`.appsdk/docs/**`、`.appsdk/skills/**`、`.appsdk/sdk-resources.json`。
- 版本 pin owner：`write_current_sdk_lock`（`main/init.rs:82-...`）与 `pin_lock`（`main/reset_governance.rs:3-140`）。
- 迁移 owner：`migrate_governance_maps`（`main/migration.rs:839-...`）由 `pin-lock` 驱动；manifest 来自 `sdk_map_migration_manifest`（`main/governance.rs:82-...`）。
- 校验终点：`verify` 要求 project SDK version == `SDK_VERSION`（`main/project.rs:85-95`）。
- Guidance 链：`guide init --mode bootstrap`（只读 proposal，`guidance/intake.rs:217-366`）→ 人工批准 → 手改规则源 → `guide compile`（`guidance/compiler.rs:133-267`）→ `guide plan/update/tour/review`（事件账本 `guidance/ledger.rs`）。
- 全局注册表链：`register_runtime`/`runtime_for_replay`（`global_registry.rs`）与 communication 事件投影（`global_registry_communication.rs`，消费者在 `communication/store_core.rs`）。

声明/注册表/实现核对结论：`main/registry.rs`（governance-map 与 registry binding）与 `global_registry.rs`（运行时/项目身份注册）职责不同，无重复 owner；`guidance` 域声明（`DOMAINS` 11 项）与实现一致，无编译期强制覆盖全部域（`workflow()` 缺失即 typed fail，属设计允许）。

## 3. 确定 findings（静态，调用链已核实）

### F1 普通 `init` 在过期 SDK pin 上先刷新 bundle，再留下混合版本状态
- 证据：`main.rs:1453-1455` → `main/init.rs:886-897`（先 `install_bundle_resources` 后 `write_current_sdk_lock`）；`install_bundle_resources` 无条件覆写当前 bundle（`main/governance.rs:247-296`）；`write_current_sdk_lock` 在 `/sdk/version != SDK_VERSION` 时 `return`（`main/init.rs:82-85`）；随后 `verify` 以 `PROJECT_SDK_VERSION_PIN_MISMATCH` 失败（`main/project.rs:85-95`）。`migrate_governance_maps` 仅由 `pin-lock` 调用。
- 影响：旧 pin 项目执行普通 `init` 会得到“新 bundle 内容 + 旧 pin/lock”的非事务部分刷新；无错误提示，直到后续 `verify` 才暴露。
- 最小修订：在任何 bundle 写入前检查 pin，旧版本 fail closed（typed `SDK_VERSION_MIGRATION_REQUIRED` 指向 `pin-lock`），或整体并入迁移事务。不建议普通 `init` 隐式执行 destructive/frozen 迁移。
- 保留保障：`pin-lock` 作为唯一迁移 owner。
- 定向验证：旧 pin 项目跑普通 `init`，断言失败前无 SDK-owned resource mutation，或显式迁移后状态一致。
- 状态：静态确定；未运行验证。

### F2 版本升级缺少 pin-lock 迁移链的刷新路径（条件性：以升级会 bump 版本为前提）
- 证据：迁移步骤集合封闭（`main/reset_governance.rs:13-21,53-83`），无 `0.1.0012-to-<next>`；`sdk_map_migration_manifest`（`main/governance.rs:82-104`）与 `historical_governance_map`（`governance.rs:6-80`）均无 `0.1.0012` 条目；`migrate_governance_maps` 在 step target != `SDK_VERSION` 时提前返回（`main/migration.rs:841-847`）。
- 影响：一旦 `SDK_VERSION` 超过 `0.1.0012`，唯一 map 刷新路径停止物化新 canonical maps，`pin-lock` 报 `UNSUPPORTED_SDK_MIGRATION`，governance maps 与 bundle 漂移。
- 最小修订：一次性补 `0.1.0012-to-<next>` 步骤 + `0.1.0012` 历史快照 + 版本列表 + 末尾刷新调用；不扩展框架。
- 定向验证：从 `0.1.0012` 项目跑 `pin-lock`，断言新 maps/record，再 `verify`。
- 状态：静态；仅在版本被 bump 时触发。

### F3 测试证据硬绑 `0.1.0012`，同一升级会使自身失效
- 证据：`rust/tests/sdk_0009_migration.rs:43-45`；`cli_smoke/part_24.rs:306-311`、`part_23.rs:44-46`、`part_01.rs:1188-1300`、`part_07.rs:371`、`part_08.rs:1318-1358`、`part_10.rs:968`、`part_02.rs:557`、`part_03.rs:607`、`part_22.rs:142` 均断言字面量 `0.1.0012`。
- 影响：版本 bump 后这些断言按构造失败；基线的“测试通过”证据不可平移到升级版本，必须与 F2 同步更新字面量并重跑。
- 最小修订：与版本 bump 同 commit 更新字面量；保留故意测试历史步骤的 `0.1.0011`/`0.1.0012` 断言。
- 状态：静态；未运行测试。

### F4（语义边界，非缺陷）`GuidanceSetupProposal` 仅供人工，不是机器门禁
- 证据：`guidance/intake.rs:217-366` 只生成 proposal（`approval_required`、`after_user_approval`、字符串命令），不持久化批准或 typed apply；`guidance.rs:922-1018` 无 `setup apply/approve`；`guide compile` 只校验声明源与机器合同（`guidance/compiler.rs:133-267`）。设计文档明确 AppSDK 不解释 prose、不保存第二套 intake truth。
- 影响：SDK 无法证明某次 durable 规则变更对应哪份 proposal/批准；`approval_required` 是协作指令，不是强制门禁。
- 最小建议：保留人工边界，但文档应声明 approval evidence 不由 SDK 强制；仅当发布合同要求可审计授权复用时才引入 typed approval/apply record。
- 状态：静态；无运行时验证。

### F5（次要，非缺陷）`flow_review` 携带未使用参数
- `guidance/review.rs:414` 的 `_workflow` 从未读取；节点/边校验仅用 `path`/`path_set`，调用方已在 `review.rs:626` 做 `assert_path`。属死参数，非缺链。

## 4. 假设（未证实，需最小验证）

- A1：`write_current_sdk_lock` 的提前返回是否被上游某处 pin 检查补偿。目前未找到该检查；假设成立则 F1 成立。最小验证：grep 全仓 `/sdk/version` 读取点 + 旧 pin init smoke。
- A2：升级是否一定 bump `release-version`。若版本不变，F2/F3 不触发；本报告按“升级会 bump”的前提标注。
- A3：`prepare → init` 路径不携带 SDK 版本信息（`prepare_project` 仅写模板文件，`main/init.rs:485-...`），故 F1 同样适用于 prepared 路径。

## 5. 删除 / 复用建议与保留理由

- 复用：`pin-lock` 作为唯一迁移 owner，普通 `init` 复用其 fail-closed 前置检查，不新建迁移框架。
- 复用：guidance 事件账本（`guidance/ledger.rs`）与 compiled manifest 已是单一真源，review/tour 是投影，不新增第二套状态。
- 删除候选（低优先、需实证）：`flow_review` 的 `_workflow` 参数。
- 不建议删除：`main/registry.rs` 与 `global_registry.rs` 职责分离，非重复；`guidance` 11 域声明与实现一致。
- 无证据不删：所有“可能冗余”的校验在无实际误用证据前保留。

## 6. 风险验证（升级前必须闭合）

- R1：旧 pin 项目执行普通 `init` 的资源 mutation 边界（F1）。
- R2：`SDK_VERSION` bump 时迁移步骤 + 历史快照 + 版本列表同步（F2）。
- R3：测试字面量与版本同步重跑（F3）。
- R4：`GuidanceSetupProposal` 的批准语义在文档中显式声明为人工边界（F4）。

## 7. 交付未知 / 未验证边界

- 未运行任何测试/构建/安装/运行时命令，所有结论均为静态。
- 安装后二进制版本、CLI 黑盒行为、daemon/collab 交互未验证（运行时 auditor 负责）。
- 未做字节级 bundle/嵌入式资源一致性校验。
- 未逐分支精读 `compile.rs`/`promotion.rs`/`verification.rs`/`communication/store_core.rs` 内部语义。
- 不输出任何产品/测试 PASS。

---

过程笔记已落盘：`/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/core/notes.md`（含 F1–F5 证据与最小修订、原错、下一步）。