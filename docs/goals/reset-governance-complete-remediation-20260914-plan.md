# AppSDK reset-governance complete remediation plan

## Execution goal prompt

```text
/goal
目标：完成 AppSDK reset-governance 整改。升级版本时只保留一个当前 reset 基线，忽略全部旧版治理残留；缺失的 SDK 自有投影按当前 scaffold 重补；移除旧版本状态后只按当前版本校验，不读取旧版本作为重建或验收前提。AppSDK 必须阻断错误交付，但不能阻断开发者继续修复、继续开发。
范围与约束：只在独立 owner worktree 修改本计划列出的文件；不新增第二套 reset 引擎；不迁移、不兼容旧 PASS/旧 witness；不把 Collab、Memory、Guidance 变成普通开发前置；不 push、不 merge、不安装、不重启、不关闭 bug。
依据：`docs/goals/reset-governance-complete-remediation-20260914-plan.md`
验收：四个 gap、文档同步、回归矩阵、完整测试、review 全部通过；reset 后普通 verify 只说明 development_ready，交付验证必须重新建立；旧版本移除后当前基线校验通过；只有一个 Conventional Commit。
直接执行本任务，不再为它生成一层提示词。
```

## Goal

让 AppSDK 能阻断错误交付，但不能阻断开发者继续修复、继续开发；治理历史可以重新建立，
质量结论必须重新证明。完成所有才算完成，不允许以“部分缺口已处理”或“已有未提交补丁”
作为本任务的完成证据。

本计划是执行真源。执行者必须按本计划完成全部四个缺口、文档同步、测试、review 和提交；
任何一项未通过，就不创建提交，也不宣称完成。

## Workspace and authorization

唯一工作树（不能切换、不能重建、不能清理其他 worktree）：

```text
/Users/fanzhang/Documents/github/appsdk-bug-integration/playground/reset-governance-transactional-20260914
branch: codex/reset-governance-transactional-20260914
base: 35132fb
```

允许修改：

- `rust/src/main.rs`
- `rust/tests/cli_smoke.rs`
- `README.md`
- `skills/appsdk-project-governance/SKILL.md`
- `skills/appsdk-migration/SKILL.md`
- `docs/goals/reset-governance-complete-remediation-20260914-plan.md`

禁止修改：

- `/Users/fanzhang/Documents/github/appsdk` 原始主树
- RouteCodex、Codex Desktop、Codex TUI、codexapp、V3、Collab 或其他非 AppSDK 项目
- 其他 executor 的 worktree、branch、dirty 文件或未提交变更

禁止动作：

- 不运行真实 Collab 初始化或 AGY
- 不 push、不 merge、不安装全局 binary、不重启 daemon
- 不关闭 bug、不伪造 receipt
- 不使用 `pkill`、`killall`、`xargs kill`、`kill $(...)`；如需要终止子进程只能按显式
  child PID 或服务级操作
- 不用 shell 写文件；编辑用 `apply_patch`

## Starting state

当前工作树已有未提交修改，位于：

```text
rust/src/main.rs
rust/tests/cli_smoke.rs
```

执行者必须先读取当前 diff，判断已有改动是否真正满足本计划；不能把“已有未提交补丁”
当成通过。若已有改动与计划冲突，在唯一 owner 内修正；不得 revert 用户或前序执行者的
非目标改动。

## Non-goals

- 不重写整个 AppSDK
- 不新增调度 daemon、第二套 reset 实现或新的治理状态机
- 不把 Collab、Memory、Guidance 变成普通开发的前置条件
- 不把普通开发流程改成必须写计划、安装、重启或全量历史补跑
- 不做未授权的 merge、push、install、deploy、bug close

## Gap 1: unify reset paths

完成标准：

1. `appsdk init <root> --fresh --discard-legacy` 和
   `appsdk reset-governance <root> --discard-legacy` 必须进入同一个 transactional reset
   owner，不接受两条语义不同的实现。
2. 两入口都获取稳定 reset lock，先执行事务恢复；已提交但未清理的事务幂等收口，未提交
   事务回滚并返回明确重试信号。
3. staging → quarantine → publish → rollback 都必须保护业务源码、runtime data、
   `active/`、`protected/`；只移除 `.appsdk`、`.appsdk-control`、声明的 generated roots
   和 SDK 自有可重建合同投影。
4. 不再因为历史存在 `reset-governance-record.json` 就输出成功。
5. 幂等绑定到同一次 operation ID：
   - 同一个 reset operation ID：恢复或返回同一结果。
   - 新的 reset operation ID：建立新的治理周期。
   - 不是“历史上存在 reset record，以后都算成功”。
6. 非 fresh reset 和 fresh init 的保留/删除集合一致，项目合同保留行为一致。

## Gap 2: one current reset baseline, no legacy migration

完成标准：

1. fresh init 和 reset-governance 共用同一 reset 基线：当前 SDK scaffold。不得保留旧
   SDK 版本白名单、逐版本迁移分支或“先修旧合同才能重建”的前置条件。
2. SDK 自有字段全部从当前 scaffold 重建，不读取旧版本值作为输入，包括但不限于：
   - `sdk/name`、`sdk/version`
   - `sdk/bundle_manifest`、`sdk/resource_record`
   - `development_scenarios.manifest`
   - `governance.record_contracts`、`governance.zone_transition_contract`
   - 缺失的 lifecycle、guidance、default governance 字段按当前合同补齐
   - SDK 自己拥有的索引、控制态、record/transition contracts 和可重建 generated
     投影
3. 项目自有字段保留，包括但不限于 `project_id`、模块、归属、保护路径、构建声明、
   Active/Protected/Generated 的真实边界。旧 SDK pin 不参与判断。
4. 能识别的旧合同：只提取项目身份、模块、归属、保护边界、构建声明，覆盖到当前 scaffold
   基线上；旧 SDK 合同投影不复制、不迁移、不作为验收输入。
5. 旧版本记录或 migration witness 缺失、版本不受当前 pin-lock 支持，都不能阻断 reset。
   旧版本移除后，只按新 staging 中的当前版本基线校验。
6. 项目拥有的安全边界无法识别时，必须 fail closed，输出明确的
   `GOVERNANCE_RESET_CONTRACT_REQUIRED:<field>`，且原状态不被污染；不得猜测可删除路径。
7. 恢复入口不能依赖旧配置符合除“真实项目安全边界”之外的全部现行要求。

## Gap 3: separate development readiness from delivery verification

完成标准：

1. `verify` 输出至少包含：
   - `development_ready`
   - `delivery_verified`
   - `baseline_status`
   - `reason`
   - `command_ok`
   - `ok`
2. reset epoch 后的普通 `verify`：
   - `development_ready = true`
   - `delivery_verified = false`
   - `baseline_status = "required"`
   - `reason = "baseline_required"`
   - 不输出可以被误读为交付验收通过的 `ok:true`
3. reset 后的豁免必须限定到“已废弃历史证据和可重建 SDK 投影”；普通 verify 跳过缺失历史
   记录时，不得跳过当前候选相关的正确性、安全、数据完整性、运行数据或 Active/Protected
   保留边界检查。
4. `verify --admission` 或实际交付验证不继承 reset 的普通 verify 豁免；缺失 compiled、
   frozen Active/Protected、历史证据图等交付必需证据时必须失败或明确
   `delivery_verified=false`。
5. “继续开发”不等于“验收通过”，也不等于“解除 frozen/protected 权限”；原有冻结/保护权限
   继续有效，只是旧验收结论不自动继承。
6. 非 reset 普通 `verify` 也不能自言自语地把 `delivery_verified` 设成 true，除非当前调用
   的实际交付/验收条件已经用当前候选证据证明。
7. 新增或调整测试，确保一个真实的重要测试失败仍阻止对应交付，reset 不能把失败变成 PASS。

## Gap 4: isolate auxiliary dependencies

完成标准：

1. 本地 fresh reset 不依赖全局项目注册成功；本地治理恢复成功后，全局注册失败输出
   `GLOBAL_PROJECT_REGISTRATION_PENDING:<error>`，不退出失败，不伪造已注册。
2. 身份冲突不能吞掉：共享写入或协作归属不明确时，对应共享操作暂停，不放松真实性检查。
3. `collab init` 调用必须有有界 deadline，超时后显式输出
   `COLLAB_INIT_TIMEOUT` 或 `COLLAB_INIT_OUTPUT_TIMEOUT`，正确回收子进程和 output pipe，
   不无限等待、不静默返回成功。
4. 辅助服务不可用只降级能力范围；独立开发可继续，但不得宣称已注册、已建立通信或已取得
   共享资源归属。
5. 测试覆盖：全局注册不可用时 fresh init 仍完成本地治理并输出 pending；Collab 挂起时有界
   返回且不伪造成功。

## Documentation sync

完成标准：

1. `README.md` 删除或修正与新流程矛盾的“新开发/debug 先写 plan 文档”的笼统提示。
2. `skills/appsdk-project-governance/SKILL.md` 的 reset 段落与实现一致：reset 是两个入口
   共用的 transactional engine，不再保留旧路径语义。
3. `skills/appsdk-migration/SKILL.md` 若提到 fresh/reset/合同重建，必须同步到同一语义。
4. 文档必须写明 reset 成功只代表 reset 成功，不隐含测试、review、交付、安装、重启或 bug
   close。
5. 文档不引入第二套规则；只修订受本次变更影响的段落。

## Tests and regression matrix

至少覆盖以下场景；每个场景都要有断言，不能只靠 `ok:true` 或进程退出码：

| 场景 | 应有结果 |
| --- | --- |
| 同一 reset operation 重试 | 恢复或返回同一结果，不重复破坏动作 |
| 新 reset operation | 建立新周期，不因旧 record 存在而 no-op |
| 任意旧 SDK pin（包括不受 pin-lock 支持的版本），records/migration witness 缺失 | 忽略旧版残留，按当前 scaffold 重建治理，不复制旧 PASS |
| 旧合同无法识别 | 明确给出需要的新合同边界；未提供批准的最小新合同时不污染原状态 |
| fresh 与 reset-governance | 保留/删除集合一致；项目身份、模块、归属、保护路径、构建声明保真 |
| staging/隔离/发布阶段中断 | 重入后一致恢复，业务资产不丢失 |
| reset 后 frozen 声明保留但当前证明缺失 | 普通 verify development_ready=true、delivery_verified=false、baseline_required；admission 不通过 |
| 当前相关重要测试失败 | 仍阻止对应交付；reset 不能变为 PASS |
| 输入、候选、依赖、环境未变化 | 复用仍有效证据，不重复同一步骤 |
| 全局注册不可用 | 本地恢复可继续，输出 pending，不伪造注册 |
| Collab 挂起 | 有界返回，输出 timeout，不伪造接入成功 |
| verify 输出 JSON | 字段语义与计划一致，`ok` 不能被误读为交付证据 |
| reset receipt | 有 `operation`、`status`、`development_ready`、`delivery_verified`、`baseline_status`、`registration_status`、`next_action`，不伪造 delivery |

## Verification gates

必须按顺序执行并全部通过；失败时从首个失效 gate 重跑受影响验证，不能只改测试绕过：

```bash
cargo fmt --manifest-path rust/Cargo.toml
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo test --manifest-path rust/Cargo.toml --test cli_smoke --quiet
cargo test --manifest-path rust/Cargo.toml --test communication_cli --quiet
cargo check --all-targets --manifest-path rust/Cargo.toml
git diff --check

# 如果仓库现有全量测试入口可用，再跑一次：
cargo test --manifest-path rust/Cargo.toml --all-targets --quiet
```

如果 `cargo test --all-targets` 包含同一测试套件且时间合理，应作为最终全量证据；至少不能
只跑新增用例而放过完整 `cli_smoke` 和 `communication_cli`。

## Review and commit

完成条件：

- 上述所有改动缺口、测试矩阵、文档同步和验证 gate 全部通过。
- 按 AppSDK review 标准 review 精确 diff；发现问题后修复并重跑受影响 gate。
- 全部通过后只创建一个 Conventional Commit。
- 回报 commit SHA、改动文件、每个缺口的完成行为、测试摘要、未验证黑盒边界、剩余风险 owner。

不得在以下情况创建提交或宣称完成：

- 任一 gap、文档、测试或 gate 未满足。
- 以“未提交补丁存在”“普通 verify 不报错”代替交付/验收证据。
- 未安装、未 merge、未 push、未重启 daemon 却声称这些已经完成。
- 一个真实重要测试失败仍被 reset 绕过。

## Completion report

最终报告必须包含：

1. 结论：本目标是否完成；未完成时列出首个未通过条件，不写成功形状。
2. 精确 commit SHA（仅在所有 gate 通过后存在）和改动文件列表。
3. 四个缺口分别如何满足，以及对应测试名。
4. 每个验证命令的实际结果摘要。
5. 未验证黑盒边界：安装后的真实 reset receipt、真实全局注册、真实 Collab、daemon restart、
   merge/push、bug close。
6. 剩余风险及 owner。
