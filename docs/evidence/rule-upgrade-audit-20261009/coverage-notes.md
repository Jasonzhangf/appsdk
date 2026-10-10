# A 覆盖补查 run notes（coverage）

- 任务：`rule-upgrade-audit-20261009`，feature `aabed6a`
- Worker：A 只读覆盖补查（run = coverage）
- 基线：`3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`（= `origin/main` = HEAD）
- 工作树：`/Users/fanzhang/Documents/github/appsdk`（integration worktree，本轮只读）
- 模式：只读静态补查。未改产品源码，未 build/test/install/restart，未 commit/merge/push，未派 agent，未注册 Collab。产物只有本 `notes.md` 与 `report.md`，无 patch。

## 0. 工作树状态（关键 caveat）

`git status --short` 非空：HEAD 仍是基线 `3dfdaf85`，但工作树已含其他 worker/controller 在途的 `0.1.0013` 迁移改动（`rust/release-version=0.1.0013`、`rust/Cargo.toml=0.1.13`、`main.rs`/`governance.rs`/`reset_governance.rs`/`canonical_map.rs`/`init.rs`、`contracts/maps/**`、`contracts/sdk-bundle.manifest.json`、`contracts/migrations/0.1.0012/`、`contracts/migrations/sdk-0.1.0012-to-0.1.0013.json`、多个 smoke 测试与 templates 更新）。

处理原则：
- core 报告（`0.1.0012` 结论）对应 HEAD/base 证据。
- 本轮补查读的是**当前工作树**（含在途改动），行号以当前文件为准。
- 不宣称当前工作树 = 基线版本；不判断在途改动正确性，只记录“存在、不归本 worker 判断”。例如 `init.rs:82-93` 的 `assert_current_sdk_pin`、`reset_governance.rs:14-24` 的 `0.1.0013` 步骤、`governance.rs:76-104` 的 `0.1.0012` 历史 map 与 `0.1.0012-to-0.1.0013` manifest 都是别的 owner 的在途实现，core 报告的 F1/F2/F3 缺口在这些 hunk 上已被触及，但本 worker 不做正确性判定。

## 1. 覆盖矩阵（活跃 owner 补查）

每类列：实际入口 / owner 真源 / 控制真源 / 关键失败与调用边 / consumer·test 边界 / 证据 / unknown。

### compile

- 入口：`appsdk compile [project] [--module <id>]`、`appsdk compile-module [project] --module <id>`（`rust/src/main.rs:1225-1255`）。
- 全项目 owner：`compile`（`rust/src/main/project.rs:443-471`）——先写 `generated/project.compiled.json`，再逐模块构建，构建前后各校验一次 control snapshot。
- 单模块 owner：`compile_module`（`compile.rs:1202-1210`）→ `compile_module_with_project`（`compile.rs:1074-1103`）。
- 控制真源：project 合同 + `assert_sdk_lock`（`compile.rs:3-65`）；预条件 `COMPILE_BLOCKED`/`FROZEN_ARTIFACT_IMMUTABLE`（`compile.rs:1324-1377`）；path/symlink/drift 门（`compile.rs:1105-1200`）。
- 失败/调用边：未确认/未冻结 → blocked；control 漂移或 symlink 组件 → typed fail；缺失 record 会把 module promotion 推进到 `architecture_stable` 时报错。
- consumer·test 边界：`cli_smoke/part_10.rs:402-476`（未确认）、`:634-652`（main 分支 mutation）、`:751-841`（control drift/symlink）、`:844-955`（artifact 路径、缺/正常 node_modules）、`:957-1035`（confirmed+lock 正例）。
- unknown（partial）：`compile.rs:481-1073` 内部逐模块 build/hash/artifact 语义未逐分支精读，标部分覆盖，不作为结论。

### promotion

- 入口：`appsdk promote [project] --to <stage>`、`appsdk promote-module [project] --module <id> --to <stage>`（`main.rs:1288-1318`）；旁支 `freeze`（`main.rs:1383-1394`）、`publish-active`（`main.rs:1396-1413`）、`begin-version`/`rehydrate-frozen`（同区）。
- 全项目阶段：`promotion.rs:3-40`，`draft→source_implemented→contract_bound→compiled→controlled_verified→architecture_stable`；到 `architecture_stable` 调 `assert_record_graph`。
- 模块：`promotion.rs:42-127`；freeze owner `promotion.rs:129-322`；active publish owner `promotion.rs:324-544`（record graph + version_base + lock/staging/active index）。
- 控制真源：project `/lifecycle/stage`；`verify` 门复用 promotion + lifecycle；plain verify 不声称交付，只有 admission 置 `delivery_verified=true`（`promotion.rs:864-1319`、`cli.rs:3-37`）。
- consumer·test 边界：`part_10.rs`（同上）；`part_12.rs:1-577`（freeze/active publish/duplicate/review admission）；`part_13.rs:1-620`（rehydrate/begin-version/历史 graph）；`part_11.rs:949-980`（freeze 预条件）。
- unknown（partial）：`promotion.rs:324-544` active publish 全失败分支未逐行；`review_gates.rs`/`review_context.rs` 分支语义仅经测试采样，标部分覆盖。

### verification

- 入口：`verify_cli`（`cli.rs:3-37`）：plain `verify`、`--admission`、`--review-admission [project] --module <id>`。
- 核心：`verify_internal`（`promotion.rs:864-1319`）；`verify_sdk_migration_record`（`promotion.rs:809-862`）；resource 校验 `promotion.rs:546-680`；reset epoch `promotion.rs:682-807`。
- 控制真源：VCS/main/worktree 门（`verification.rs:359-528`）；remote receipt（`verification.rs:530-544`）；evidence 过期/因果序（`verification.rs:558-874`）；`assert_pre_review_validation_gate` 含 deploy install/restart + blackbox/whitebox 因果（`verification.rs:666-874`）。
- consumer·test 边界：`part_07.rs:596-762`、`:916-1070`、`:1214-1307`、`:1344`；`part_22.rs:236-781`（review context/review gates）；`part_24.rs:523-595`（review 失效）；`part_12.rs:82-240`（admission source/artifact/candidate drift）。
- unknown（partial）：`verification.rs:36-216` schema assert 与 `assert_record_schema` 未全读；`merge_gates::assert_record_graph_mode`（`447-860`）部分采样，标部分覆盖。

### lifecycle / producer

- 入口：`produce-lifecycle-records`（`main.rs:1320-1337`）、`produce-lifecycle-chain`（`main.rs:1358-1381`）、`retire-lifecycle-records`（`main.rs:1339-1356`）。
- records owner：`lifecycle_chain.rs:3-552`；chain dispatcher `verification.rs:3-18`（phase ∈ architecture|effectiveness|merge|promotion，非法即 `PRODUCER_PHASE_INVALID`）；architecture/effectiveness/merge/promotion 分别 `lifecycle_closure.rs:421-624 / 626-796 / 837-977 / 979-1232`。
- merge 无独立 CLI，merge record 由 `lifecycle_chain_merge` 写，门由 `merge_gates.rs:59-123`。
- 控制真源：producer lock `producer.rs:395-432`（`flock` 非阻塞 → `PRODUCER_BUSY`）；durable 事务 `producer_commit.rs:340-412`（staging/marker/hard-link/rollback），recovery `producer_commit.rs:98-275`；attempt ledger `lifecycle_chain.rs:672-857`；candidate/ancestry/source 身份 `verification.rs:394-528`、`lifecycle_chain.rs:596-620`。
- consumer·test 边界：`part_04.rs`（map 投影/reentry/stale replacement/attempt ledger）；`part_05.rs`（phase/parallel/collab live closure/candidate tree/ancestry）；`part_06.rs`（tampered maps/partial recovery/missing/shadowed）；`part_11.rs:1-575`（clean worktree/baseline/nested）；`part_10.rs:1-35`（lock busy）。
- unknown：`lifecycle_chain.rs:3-552` 全分支仅由测试采样；collab live closure 属 Collab 范围（runtime advisory owner），非 AppSDK release 阻断。

### merge

- writer：`lifecycle_closure.rs:837-977`；单 merge 门 `merge_gates.rs:59-123`（candidate 祖先、tree 身份、mainline ref 解析、`RECORDED_MERGE_NOT_ON_MAINLINE`）；并行门 `merge_gates.rs:127-403`（采样）。
- 控制真源：`merge_gates.rs` 的 `assert_record_graph_mode`（`447-860`，部分采样）。
- consumer·test 边界：`part_05.rs:422-857`（merge/promotion mismatch、parallel first create/reuse/collab closure）；`part_12.rs`（经 verify/freeze 走 record graph）；`part_13.rs`（rehydrate 历史 merge/mainline）。
- unknown：`merge_gates.rs` 全 graph 分支未读 EOF；无独立 CLI（预期）；remote `ls-remote`/mainline receipt 只在 parallel 路径，全 receipt consumer 未验证。

### reset

- 入口：`reset-governance [project] --discard-legacy`（`main.rs:1218-1224`）；内部 `reset-staging-scaffold`（`main.rs:1415-1428`）→ `migration.rs:64-259`。
- 主 owner：`reset_governance`（`reset_governance.rs:315-318`）→ `reset_governance_internal`（`bug_tracker.rs:3-47`：非 main 分支、lock、recover、clean worktree）。
- 事务：`reset_run.rs:3-105`（staging build）、`reset_run.rs:219-509`（transaction）；lock `reset_transaction.rs:49-91`；recovery `reset_validate.rs:512-622`；marker/相对校验 `reset_transaction.rs:409-520`；receipt mode `reset_governance.rs:191-230` + `promotion.rs:682-807`。
- 失败门：`RESET_REQUIRES_NON_MAIN_WORKTREE`、`RESET_REQUIRES_DISCARD_LEGACY`、`GOVERNANCE_RESET_BUSY`、`RESET_REQUIRES_CLEAN_WORKTREE`、symlink/security 检查。
- consumer·test 边界：`part_01.rs:538-982`（幂等、committed marker recovery、嵌套）；`part_02.rs:1-1397`（fresh init transaction/recovery）；`part_03.rs:1-1200`（fresh/reset 拒绝、busy、main/dirty/symlink、prepare）；`part_07.rs:1029`（mode-aware receipt）；`part_24.rs:456-495`（reset 保留授权历史）。
- unknown：`reset-staging-scaffold` 无直接 CLI 测试，仅经公开 reset/fresh-init 间接覆盖，标静态覆盖 + 直接子命令 UNVERIFIED。

## 2. 可整合闭环

active owner（compile / promotion / verification / lifecycle+producer / merge / reset）各自有唯一 CLI 入口、唯一控制真源（project 合同 + `.appsdk-control` + records + git 身份）、typed 失败与对应 smoke 测试边界。覆盖表足以支撑 controller 组合 release 批次，不需本 worker 新增补链或框架。

## 3. 历史 advisory（精准 owner，静态不冒充因果）

- core F1（旧 pin 普通 `init` 先刷 bundle 后留旧 lock）：runtime 已由 upgrade-observation 复现确认；core 报告 owner = main/init 写前 fail-closed。**当前工作树 `init.rs:82-93` 已出现 `assert_current_sdk_pin`（他人在途），本 worker 不判正确性**。
- core F2/F3（迁移链硬编码、测试字面量绑 `0.1.0012`）：owner = 版本组合 owner；**当前工作树已扩 `0.1.0012-to-0.1.0013` 步骤与测试字面量更新（他人在途），不判正确性**。
- foundations F1-F4：Communication/Memory owner，历史 advisory，仅在破坏 install/migration/verify 时才是本次阻断。
- runtime F1-F7：Collab owner，非 AppSDK release 阻断。
- rules F1-F9：installer/rules scope，由 controller/B/D 处理，本 worker 只标注集成边界。

## 4. 未验证边界（本 worker）

- 未运行任何 build/test/install/runtime；全部为静态。
- 上述 partial：`compile.rs:481-1073`、`promotion.rs:324-544`、`verification.rs:36-216`、`merge_gates.rs:447-860`、`review_context/review_gates` 分支语义、`reset-staging-scaffold` 直接 CLI。
- 不输出任何 PASS；不把静态分析当 runtime 因果。
