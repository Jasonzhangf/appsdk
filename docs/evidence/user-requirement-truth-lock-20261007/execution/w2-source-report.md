# W2 report：权威需求到 review 的来源、版本与黑盒合同

状态：DONE（只读侦察）。本文件是 W2 交付物。产品、配置、Skill、Git、daemon 未改动，未 build/test/install，未创建 agent。

## 0. 基线与方法

| 项 | 值 | 证据 |
| --- | --- | --- |
| worktree | `/Volumes/Intel/playground/appsdk/authoritative-review-template-20261007` | `pwd` |
| branch | `codex/authoritative-review-template-20261007` | `git branch --show-current` |
| base/HEAD | `c3c0c8df79e69534fe30c92db61328824473d5c0` | `git rev-parse HEAD` |
| 候选设计 | `docs/design/user-requirement-truth-lock.md` sha256 `cf2d07642e5572c8ea2b76989189c8b676b747da951fa94b205cb069b3bac4cb` | `shasum -a 256` |
| goal 提示词 | `docs/goals/user-requirement-truth-lock-goal.md` sha256 `c90c67b86b1ec08988dc3075b47ad7f2338577a4c58c6b747cffb1228296d621` | `shasum -a 256` |
| SDK bundle 版本 | `0.1.0010` | `contracts/sdk-bundle.manifest.json` |
| 本仓库治理状态 | 根目录无 `.appsdk/project.json`，SDK 源码仓库默认不是 managed project | `test -e .appsdk/project.json` → absent |

说明：主树 `/Users/fanzhang/Documents/github/appsdk` 设计草案 sha256 为 `0a1ac985...`，与候选副本不同；候选副本多出 `assemble_review_packet` 节点，属 parent 的较新修订。以下结论以候选副本为准。方法：只读 `rg`/`sed`/`shasum`/`diff`，无写入产品路径。

## 1. 已存在的保证（可直接复用，不必重建）

1. **目标记录结构与确认门（弱）** — `contracts/records/goal-clarification-record.schema.json` 保存 `raw_request`、`understood_objective`、`acceptance_criteria`、`non_goals`、`scope`、`status`、`confirmed_by`、`confirmed_at`。`rust/src/main/registry.rs:696`（`validate_goal_contract`）校验字段形状；`:785` 起仅要求 `status ∈ {confirmed, admitted}`、`confirmed_by`/`confirmed_at` 非空、`admitted` 时 `scope` 存在。
   - 关键限制：`confirmed_by` 只是一个非空字符串，agent 可自填。它**不证明**用户身份或授权（设计 G2 已承认）。不得把 `confirmed_by` 非空当作真实认证。
2. **目标文件版本哈希** — `rust/src/main/goal.rs:386-394` 在 `goal subscribe` 时计算 `goal_revision = sha256(goal 文件字节)`，绑定进 `goal.json` 记录；`verified_goal_master` 要求 live Collab master。用途是长程重生成/订阅，**不是**长期需求真相锁；它绑定"哪个文件"，不绑定"谁授权改动"。
3. **编译控制快照** — `rust/src/main/compile.rs:1105` 起 `compile_control_snapshot` 把 `project_contract`、`goal`、`development_scenarios`、`zone_transition_contract` 及声明的 `record_contracts` 纳入快照哈希。检测漂移，不授权改动。
4. **Guidance 版本绑定** — `rust/src/guidance.rs:401` 起 `rule_context` 绑定 `project_contract_hash`、`goal_hash`、`guidance_manifest_hash`、`source_commit`、`source_tree_hash`、`scope_hash`、`scope_state_hash`、`owner`。已提供"精确版本 + 作用域"漂移检测骨架。
5. **ReviewRecord 候选/证据绑定** — `contracts/records/review-record.schema.json` 绑定 `promotion_id`、`fix_candidate_id`、`pre_review_validation_id`、`reviewer{adapter,identity}`、`verdict`、`evidence_ids`、`reviewed_commit/tree_hash/diff_hash/artifact_hash/scope_hash` 及四张 map 哈希。**无需求版本/来源字段。**
6. **Review 身份不可重算替换** — `rust/src/main/producer.rs:785` `assert_lifecycle_chain_review_identity` 从 `promotion_id + fix_candidate_id + reviewer + verdict + evidence_ids + project_bindings` 重算 `review_id`；不符即 `ARCHITECTURE_REVIEW_IDENTITY_MISMATCH`。`rust/src/main/lifecycle_closure.rs:527-615` 生成端同源。改任一输入即改 ID。
7. **Review 准入门** — `rust/src/main/review_gates.rs:3` `verify_review_admission`：`assert_goal_confirmed` → 读 module → `assert_pre_review_validation_gate` → 证据/因果/哈希校验，成功打印 `{"ok":true,"gate":"review_admission",...}`，失败 exit 1 并写 stderr 错误码。
8. **EvidenceRecord 绑定** — `contracts/records/evidence-record.schema.json` 绑定 `phase`、`kind`、`source_commit`、`scope`、`producer{adapter,identity}`、`artifact_hash`、`execution_surface`、`environment_id`、`entrypoint`、`expires_at`。**无需求版本引用。**
9. **SDK 分发链** — `contracts/sdk-bundle.manifest.json`（v0.1.0010）已分发 review/evidence/goal 契约与 `skills/appsdk-project-governance/**`（含 `references/review-delivery.md`）。`rust/src/main.rs:80` `SDK_BUNDLE_RESOURCES` 用 `include_str!` 嵌入；`rust/src/main/governance.rs:152-291` `install_bundle_resources` 写入 `.appsdk/{contracts,docs,rules,skills}` 并生成 `.appsdk/sdk-resources.json`（含每资源 digest）。新模板必须同时进 `SDK_BUNDLE_RESOURCES` 与 manifest，否则 `SDK_BUNDLE_MANIFEST_MISMATCH`/`SDK_BUNDLE_RESOURCE_SET_MISMATCH`（W1 owner）。
10. **reviewer 输出合同（已有承载）** — `review-standards.md`「权威用户需求核验」要求在既有 `module_boundary_evidence.resources` 记录需求来源/版本，在 `edges`/`gates` 记录条目到实现/证据/结论；**不新增输出字段**。故需求来源/版本可用现有 review 输出字段承载，无需先改 schema。

## 2. 真实缺失

| 缺口 | 现状 | 影响 |
| --- | --- | --- |
| 用户身份/授权认证 | `confirmed_by`/`confirmed_at` 只是 agent 可写的非空字符串（`registry.rs:785`）；无用户 vs agent 区分证明 | 无法证明"只有用户能改需求" |
| 长期需求版本集合/历史/替代/撤销 | 无独立存储与 API；仅有单任务 `goal.json` | 无逐条版本、无前后关系、无撤销记录 |
| ReviewRecord/EvidenceRecord 需求版本引用 | schema 无该字段 | 审查结果无法机器绑定"审的是哪个需求版本" |
| SDK 自有正式 review 模板与组装入口 | 只有通用规则 + `review-delivery.md` 生命周期指引；无"按权威需求组装 review 包"的公开入口 | agent 需手拼材料，易漏需求或改原文 |
| T1–T16 需求锁黑盒矩阵 | 设计中列为测试合同，**尚未执行**，无产品实现 | 无行为证据 |
| 物理防篡改 | 同权限 shell 可改 `.appsdk` 下记录；无写权限隔离证明 | 不能宣称强锁，只能宣称"消费前漂移检测 + 受控 API 拒绝" |

## 3. 最小 review context 合同（建议 parent 冻结）

目标：让 reviewer 在**不看作者摘要、不依赖本机全局 Skill** 的前提下逐条核验权威需求。承载分三类，禁止混用。

| 输入 | 语义 | 承载位置（引用 / 内容 / 控制事实） |
| --- | --- | --- |
| 需求原文与条目 | 用户原话 + 规范化条目 ID/文本 | **内容**：从权威需求 owner 加载；agent 只能引用，不能自由填写替代文本 |
| 需求 ID + 精确生效版本 | 稳定身份 + 版本/revision | **控制事实**：来自 typed 需求记录；不得从日志/payload 重建 |
| 来源引用 | 指向权威记录与用户原文的路径/指针 + 哈希 | **引用**：`module_boundary_evidence.resources` |
| 变更前后 + 用户授权 | 旧版本、新版本、用户明确变更指令/来源 | **控制事实**：来自受控授权入口；agent 自报 `approved_by` 不接受 |
| scope/candidate | module_id、candidate commit/tree/diff/scope_hash、base、允许/禁止路径 | **引用**：候选记录；进入 review 包 |
| 验收证据 | 公开入口黑盒 evidence_ids，绑定 candidate/artifact/env/entrypoint | **引用**：`evidence_ids`；内容为证据记录 |

原则：
- **引用**只传指针 + 哈希；**内容**由 owner 在组装时加载；**控制事实**（授权、版本、来源、状态）只走 typed control resource/record，绝不写入业务 payload、`metadata`、调试日志或隐式上下文（对应全局 AGENTS L2 控制真相边界）。
- 生成的完整提示词是**派生审查产物**，必须回链权威版本；它不是新的需求真源。
- 任何 backend 消费同一份 AppSDK 组装的材料，不能依赖本机全局 Skill 恰好含相同文字。
- 复用现有字段优先；仅当 `module_boundary_evidence` 无法表达时才扩展 ReviewRecord，不另建 review 数据库。

## 4. 公开黑盒验收矩阵

现有公开入口（`rust/src/main/cli.rs`）：`appsdk verify --review-admission [project] --module <id>`、`appsdk produce-lifecycle-records`、`appsdk produce-lifecycle-chain`、`appsdk goal subscribe|status|cancel|prompt`、`appsdk compile`、`appsdk promote-module`。**尚无**"按需求组装 review 包"的命令；下表 `<assemble>` 指设计 7.1/7.2 待新增的公开组装入口，名称由 parent 冻结。所有断言基于 stdout/stderr 与退出码，不用源码字符串断言。

| 场景 | 操作 | 公开入口 | 期望外部结果 |
| --- | --- | --- | --- |
| 成功 | 在新建 managed consumer fixture（有 `.appsdk/project.json`）确立一条需求并提交匹配 scope + 黑盒证据 | `<assemble>` → `appsdk verify --review-admission <proj> --module <id>` | review 包含需求 ID/版本/来源、scope、evidence_ids；准入 stdout `{"ok":true,"gate":"review_admission",...}`；需求内容未变 |
| 缺需求 | 删除/置空 `.appsdk/goal.json`，或 `status` 非 confirmed | `appsdk verify --review-admission` | exit 1，stderr `MISSING_GOAL_CLARIFICATION_RECORD` 或 `GOAL_NOT_CONFIRMED:<status>`（已存在，`registry.rs`） |
| 篡改 | 改持久化 review-record 的 `project_bindings`/`evidence_ids`/`verdict`；改 candidate `tree_hash`；改 evidence `producer` | `appsdk verify --review-admission` | exit 1，`ARCHITECTURE_REVIEW_IDENTITY_MISMATCH` / `FIX_CANDIDATE_TREE_MISMATCH` / `*_EVIDENCE_MISMATCH`（`part_06.rs`、`part_12.rs` 已覆盖） |
| 源码/产物漂移 | 改候选受控源或已构建产物 | `appsdk verify --review-admission` | exit 1，`CANDIDATE_CONTROLLED_SOURCE_DRIFT` / `REVIEW_ADMISSION_ARTIFACT_SOURCE_DRIFT`（`part_12.rs` 已覆盖） |
| 缺来源 | 需求记录存在但来源引用缺失/不可读 | `<assemble>` / 准入门 | 明确 `*_SOURCE_MISSING`/`REQUIREMENT_SOURCE_UNREADABLE`；不得用空 findings 代替（设计 7.1；**待实现**） |
| 陈旧版本 | 用户合法改版后复用旧 candidate/旧 evidence/旧目标绑定 | `<assemble>` + 准入门 | 明确陈旧阻断；按新版本重验，不得改原需求（设计 T15；现有类似：`EXPIRED_EVIDENCE_RECORD`、`ARCHITECTURE_REVIEW_MAP_STALE`） |
| 未授权降验收 | agent 改 `acceptance_criteria` 或删硬约束、伪造 `confirmed_by` | 需求写入口 / `<assemble>` | 写入拒绝、无生效回执；准入不得 PASS（**依赖可信授权入口，当前 UNVERIFIED**） |
| 不同 backend | 同一 review 包分别交 Codex 与 AGY reviewer | `<assemble>` → 各 backend → 准入门 | 两 backend 读同一 AppSDK 组装材料；`reviewer{adapter,identity}` 进 `review_id`；移除本机全局 Skill 不改变包内容与结论 |
| 新项目 / SDK 分发 | `appsdk init/new` 到全新 consumer 项目 | 安装后读 `.appsdk/sdk-resources.json` + 模板 | 资源清单含模板条目与 digest；模板可在无全局 Skill 环境加载；重装 digest 一致（W1 owner，`governance.rs:243`） |

说明：`dagpipe graph validate` 只证明拓扑；`appsdk verify --review-admission` 成功也不等于需求锁建立（G4）。二者都不能替代上表行为证据。

## 5. 无法证明的能力 / 权限缺口（不得宣称完成）

- **用户认证**：`UNVERIFIED`。现有 `confirmed_by`/`confirmed_at` 不区分用户与 agent。
- **物理防篡改**：无写权限隔离证明。只能主张"受控 API 拒绝 + 消费前漂移检测"，不能主张强锁。若 agent 持有授权凭据或权威存储写权限，不得宣称覆盖该威胁。
- **长期需求版本存储/历史/撤销 API**：不存在。
- **SDK 正式 review 模板 + 组装入口**：不存在（仅通用规则 + `review-delivery.md`）。
- **T16 产品证据**：无。单独测 Codex/AGY 字符串拼接不算产品验收。
- **SDK 源码仓库自治理**：根无 `.appsdk/project.json`，验收必须用新建 consumer fixture，禁止自动 init 本仓库。

## 6. 当前可用增量与停止条件

- **可交付增量（G9/T16）**：SDK 自有正式 review 模板 + 从权威需求记录加载内容/版本的组装入口 + review 准入/记录绑定。落点：SDK bundle 资源（W1 接线）、`review_gates.rs`、ReviewRecord 验证 owner。
- **必须 load-bearing**：新模板进 `SDK_BUNDLE_RESOURCES` 与 manifest；组装入口从 owner 加载需求内容（不接受 agent 自由填写）；准入门在需求来源/版本/证据不匹配时阻断。
- **停止条件**：不得用"先自填批准，后补认证"交付；不得把物理防篡改或模型服从宣称完成。缺可信授权入口时，G1–G8 需求写权限隔离停在能力确认，只交付 G9/T16 的模板与组装 gate，并显式标注授权能力 `UNVERIFIED`。
- **不新增**：无依据的状态机、授权服务、第二套需求真源、review 数据库。

## 7. 证据索引

- 设计/目标：`docs/design/user-requirement-truth-lock.md`、`docs/goals/user-requirement-truth-lock-goal.md`
- 契约：`contracts/records/{goal-clarification,review,evidence,pre-review-validation}-record.schema.json`、`contracts/prepare.schema.json`、`contracts/sdk-bundle.manifest.json`
- 源码：`rust/src/main/registry.rs:663-806`、`rust/src/main/goal.rs:289-400`、`rust/src/guidance.rs:378-410`、`rust/src/main/compile.rs:1105`、`rust/src/main/review_gates.rs:1-360`、`rust/src/main/producer.rs:785-837`、`rust/src/main/lifecycle_closure.rs:527-615`、`rust/src/main/init.rs:390-461`、`rust/src/main/governance.rs:152-291`、`rust/src/main.rs:80`
- 测试：`rust/tests/cli_smoke/part_12.rs`（准入/漂移/篡改/因果/过期）、`part_06.rs`（review 身份篡改）、`part_05.rs:1296-1435`（project_bindings 绑定）
- reviewer 合同：`/Users/fanzhang/.agents/skills/codex-review/review-standards.md`「权威用户需求核验」
