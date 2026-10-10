READY。本计划推进一个完整 milestone：**AppSDK 0.1.0013 规则升级、必要实现与消融、正式安装、独立审查、main 集成和公开 release**。内部节点不分别提交或审查。源码仓库保持非 managed consumer，不执行 init、control、reset 或 Collab 注册。

规划节点、证据和取舍已写入 [planner/notes.md](/Volumes/Intel/playground/appsdk/.worker-runs/rule-upgrade-audit-20261009/planner/notes.md)。本正文由父 CLI 保存为 `planner/plan.md`。Planner 未实现、未派 worker、未自审，也未运行产品测试、安装或发布。

**1．基线、事实与计划边界**

任务 `rule-upgrade-audit-20261009`，feature `aabed6a`。工作树 `/Users/fanzhang/Documents/github/appsdk`，分支 `codex/rule-upgrade-audit-20261009`，输入基线 `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`。

已接收 core、runtime、foundations、rules 四份 observation。规划时 `upgrade-observation/report.md` 尚未取得，已读其完整 `notes.md` 和实验结论；不等待无关范围。

可直接复用的当前证据：

- 源码和 canonical `/Users/fanzhang/.cargo/bin/appsdk` 为 `0.1.0012`；GitHub Latest `v0.1.0007` 是父 observation 的发布快照，发布边界再刷新。
- canonical CLI 已复现旧 `0.1.0011` consumer 普通 init：退出 0，刷新资源到 `0.1.0012`，project/lock 保持旧版本，随后 verify 仍拒绝。
- 当前版本 consumer 普通 init 无文件变化，verify 成功；同一旧 fixture 经正式 pin-lock 后版本一致且 verify 成功。
- 首次写入发生在 `ensure_governance_layout → bootstrap_contracts`。因此旧 pin 检查必须位于该写入之前，不能只放在 bundle installer 内。
- 当前唯一 workflow 无 installer 测试；installer 测试固定期待 `0.1.0010`，与当前版本不符。
- 正式 installer 安装 AppSDK、project-memory 和三个 SDK-managed Skills。它不安装 Collab。
- 根目录没有项目 `AGENTS.md`。本仓库现有模板、机器合同和源注册表仍须按实际消费者处理，不能把 consumer 模板当源码仓库运行状态。

四份 auditor 的静态判断不等于因果证明。除旧 pin init 外，不将历史疑点直接纳入 debug 修复。

**2．最小充分 DAG**

```text
用户目标与会话授权
  ├─ source intake / 模板 / SDK-managed Skills
  │    → 读取实际上层规则、本地规则、CI/hook
  │    → 提出删除 / 合并 / 缩域 / 必要新增
  │    → 已授权差异直接执行；未覆盖差异取得批准
  │    → 项目规则与实际入口落实
  │         └─ Guidance 被选用时才声明、compile、验证
  │
  └─ release-version / bundle / 历史 maps / 迁移
       → 旧 pin 普通 init：写前明确拒绝
       → pin-lock：连续迁移并保留历史与项目数据
       → 正式 installer
       → canonical binary + installed Skills + consumer 黑盒
       → 完整 release 候选验证
       → controller 有效独立 milestone Review
       → 一次正式提交 / main 集成 / remote 与 CI 回执
       → AppSDK release assets / 下载核对 / 公开回执
       → issue 收口 / 证据归档 / 自有资源回收
```

这张图覆盖必要入口、控制边界、owner、资源和终点。规则审计由 agent 执行，SDK 提供建议与入口；SDK 不据此宣称项目已落实。

现有 `docs/dagpipe/sdk-pin-history.graph.json` 已描述 pin-lock 的历史认证、当前迁移、保留核验和版本发布链。此次普通 init 写前 guard 不改变该图拓扑。**不新增 graph、规则注册表或永久状态机。**只有执行中发现影响本次验收的图错误，才由图 owner 修改对应图并校验一次。

现有 `contracts/maps/**` 仅更新实际新增路径、owner、调用或验证绑定。不要为“完整审计”补一套新治理骨架。

**3．覆盖补查与 advisory 分流**

审计完成的标准是覆盖当前活跃模块的入口、owner、真源、关键调用边、失败边界、消费者和验证入口，并明确未知；不要求内部逐分支穷尽。

| 范围 | 已有证据 | 必须补查或交付 |
|---|---|---|
| Core、Guidance、注册表、迁移 | 主要文件全读；部分 lifecycle 文件仅扫描 | 补查 compile、promotion、verification、producer、merge、reset 的活跃入口和关键边界；列出覆盖缺口 |
| Communication、Memory、DAGPipe、long-horizon 基础 | foundations 静态覆盖 | 复用；发布完整测试覆盖 AppSDK 所含能力。历史疑点保持 advisory |
| Collab | runtime 静态结构覆盖 | 保留报告与 owner 待办；本次不安装、重启或发布 |
| 模板、Skills、合同、installer、CI | rules inventory 和 consumer 线索 | 落实规则升级、必要消融与安装/release 验收 |

新增的结构补查是 READY 节点，不需要产品写入。发现实际阻断时，只停其下游。

历史待办明确归属：

- **Collab owner**：runtime F1 CLI/MCP subagent 接线、F2 reset 文档、F3 authority 文档、F4 CI、F5/F6 未消费实现、F7 图描述。报告中的“发布阻断”仅适用于包含 Collab 的发布，不能扩展为本次 AppSDK 阻断。
- **Communication owner**：foundations F1 缺失 root 创建行为；F2 reserved/replay 兼容面。进入修复前须公开入口复现并查兼容消费者。
- **Memory owner**：foundations F3 schema 版本、F4 空 node group 与声明。进入修复前须确认公开输出及必要消费者。
- **Guidance owner**：core F5 未用参数，可在该函数被本次修改时顺带消融；否则留待办。

若上述问题实际破坏本次安装、迁移、规则落实、安全或证据真相，记录复现和影响，向 controller 请求调整相应范围；不自行把全部历史问题塞入 release。

**4．一个 milestone 内的最小任务与 ownership**

所有执行者只修改分配路径，不能覆盖其他执行者改动。controller 负责共享文件、候选组合、安装、Review、集成和发布。

| 节点 | 依赖 / owner | 具体动作与 allowed paths | 禁止范围 |
|---|---|---|---|
| A：覆盖补查 | READY；只读 audit owner | 补查 core 报告未充分覆盖的活跃链；结果写自己的 run notes | 产品、全局规则、其他 notes |
| B：规则升级 | READY；rules owner | `sdk-skill-sources/**`、`templates/minimal/AGENTS.md`、相关 design/architecture 文档；删合并缩域旧规则，明确日常/release、授权复用、Guidance 可选 | 全局 installed Skills、Collab 产品源码 |
| C：intake 与 init | READY；core owner | `rust/src/guidance/intake.rs`、`rust/src/main/init.rs`、对应 `cli_smoke/part_07.rs`、`part_13.rs`，必要新用例文件 | 其他运行时模块、批准账本、新 apply 框架 |
| D：CI 与 installer 验证 | READY；delivery owner | `.github/workflows/verify.yml`、`scripts/install-global-appsdk.sh`、`scripts/tests/test-install-global-appsdk.sh`、README 发布范围说明 | Collab installer/daemon、共享配置、hook 豁免 |
| E：版本、迁移、资源组合 | B/C/D 接口稳定后；integration owner | `rust/release-version`、Cargo 版本/lock、`main.rs`、`main/governance.rs`、`main/canonical_map.rs`、`main/reset_governance.rs`、`contracts/maps/**`、必要 migration/bundle/template 版本、受影响版本测试 | 重写历史快照、项目自有 maps、全部历史版本字符串批量替换 |
| F：完整候选验收 | A–E 完成；controller/validation owner | 受影响开发测试、release 全套、正式安装、canonical 黑盒、证据绑定 | skip、弱化断言、worktree binary 冒充 runtime |
| G：Review → 集成 → 发布 → 回收 | F 通过；controller | 当前 review 合同、正式 Git、CI、GitHub release 和自有资源回收 | prose PASS、绕 hook、强推、他人资源 |

B、C、D 可并行，A 同时补查。E 独占共享 `main.rs`、bundle manifest、迁移和版本测试，避免共享写入冲突。

**5．每项实现和行为验收**

**B/C：升级审计与授权语义**

沿用 `GuidanceSetupProposal`，同步修改现有 `questions`、`agent_instruction`、`proposal_schema`、`after_user_approval` 和 `next/readiness` 文案，使它们表达同一合同：

- 先读实际生效的上层规则、本地 AGENTS/Skills、测试命令和 CI/hook，再比较 advisory 模板。
- 每项差异说明位置、owner、删除/合并/缩域/新增动作、依据、保留保障和实际入口影响。
- 已有会话授权覆盖差异时复用；只批准未覆盖部分。
- bootstrap 保持只读，不从 prose 自动推断授权，不增加批准记录或机器审批器。
- Guidance 未选用时，可完成相同规则审计和 CI/hook 修改；不要求声明 Guidance 或 compile。
- init/pin-lock 输出引导相关升级审计，但不把重复 init、无关版本变化变成整仓审计触发器。

`approval_required: true` 等现有输出不能继续表达“必须再次批准”。可改为明确的条件语义；测试同时覆盖 proposal 的只读结果与各字段的一致性。SDK 不认证自然语言授权范围，由执行 agent 和独立 reviewer 核对原文。

公开行为验收：

1. 无 Guidance consumer 能取得只读审计建议；调用前后无 durable 文件变化。
2. 已有 Guidance consumer 能取得升级比较建议；不自动写规则或重编译。
3. 模板、三个 installed Skills、intake 输出均要求主动消融和检查实际 CI/hook。
4. 未覆盖授权部分保持待批准；不得由测试伪造用户批准。
5. 选用 Guidance 后，仍使用已有声明和 compile 入口；未选用时没有新增强制状态。

**C：旧 pin init guard**

在已有项目的普通 init 进入任何写入前读取并检查 SDK pin。版本不匹配时返回明确 typed error，指向正式 pin-lock。保持 fresh/reset 的现有授权路径，不让普通 init 隐式迁移。

红→绿验收：

- 使用正式生成的 `0.1.0012` consumer：升级后的普通 init 非零退出，错误说明迁移入口，完整 consumer 树无新增或修改。
- 当前版本 consumer 普通 init 成功，保留 AGENTS、项目 Skill、requirements、maps、records、Active、Protected。
- 经 pin-lock 后再 init 成功；重复 init 不因相同模板再次要求整仓审计。
- 保留 symlink、linked-worktree 和非法项目拒绝测试。

现有旧 pin canonical 实验作为红证据直接复用；新 guard 回归在测试中红→绿。若需宣称根因结案，隔离候选中恢复 guard 前状态应复现同一污染，随后恢复修复；不重装旧 binary 干扰共享 runtime。

**E：版本与连续迁移**

使用发布版本 `0.1.0013`，Cargo package 为 `0.1.13`。这是 SDK payload 和行为升级，不能沿用 `0.1.0012`。

新增 `0.1.0012-to-0.1.0013` 步骤：

- 从基线 canonical maps 保存 `0.1.0012` 历史快照。
- 增加 manifest、embedded resource 和历史 lookup。
- pin-lock 顺序保留既有步骤，再执行新终步；不得把 `0011→0012` 简单改名。
- 更新当前版本断言；故意验证历史迁移的版本和历史哈希保持原值。
- 新步允许 maps 内容相同；这表示版本连续，不要求造无意义 maps 改动。
- 新 consumer 不生成历史迁移记录；当前版本 pin-lock 幂等。
- `0011` fixture 连续经过 `0012` 到 `0013`，旧记录与 requirements 原字节保留。
- `0012` consumer 到 `0013` 后，project、lock、resources、bundle 一致，verify 成功。
- 错 binary、篡改 witness/maps、unsupported pin 仍明确拒绝。

**D：日常 CI 与 release 完整覆盖**

采用现有 GitHub workflow 的小型组件路径选择，不建 scheduler。

- 文档/Skill/模板文本：格式、链接、资源一致性及受影响 intake/bundle 用例。
- Rust 行为：按明确 owner 和调用链运行受影响目标；不能可靠缩小的共享入口、版本、runner 或依赖变化，运行完整受影响包并记录原因。
- DAGPipe 变化：DAGPipe 完整目标及 AppSDK 必要 consumer。
- Collab-only 变化：可在同一 workflow 加独立组件 job，不把其安装、重启或发布接入 AppSDK release。
- release 候选：AppSDK Rust 全套、DAGPipe 完整目标、合同/模板/资源检查、两 binary build、installer 测试和 canonical consumer 验收。

release 触发使用 `workflow_dispatch` 与版本 tag；日常 push/PR 的 release build/smoke 不再无条件运行。版本/bundle/migration 修改属于广泛影响，应选择完整 AppSDK 包。选择器缺基准或遇未知共享路径时，显式扩大对应包范围，不能空跑后宣称 PASS。

installer 测试从 fixture/runtime 或 release-version 取得期待值，保留安装幂等、失败构建不替换、缺 Skill 不替换、无关文件保留等断言。其 fake cargo 仅证明安装事务，正式安装必须另有真实 build 和 canonical 证据。

**6．消融决定与保留理由**

| 项目 | 本次动作 | 保留或删除依据 |
|---|---|---|
| 旧模板 transition 副本、orphan schema | 精确消费者核对后删除，并修失效引用 | 无当前消费者且模板内容已漂移；canonical transition 保留 |
| 模板同义控制真源条目 | 合并 | 同一保障无需重复正文 |
| SDK Skills 中重复 Collab 操作流程 | 缩域并引用 Collab owner | SDK-only 升级不需要冻结、重启、身份迁移；涉及的失效 reset 示例须修正 |
| root `contracts/**` 与 `.appsdk/contracts/**` | 保留职责边界 | 前者被项目声明和 compile 消费；后者是 SDK 分发参考。删除会破坏现有消费者 |
| `.appsdk/rules` 与 Skill 生成副本 | 本次保留兼容路径，明确唯一 source Skill | 当前文档、bundle 和 smoke 使用该路径；无消费者迁移证据，不能直接删除公开资源 |
| module-registry 不在 SDK overwrite payload | 保留 project-owned scaffold | 自动覆盖会损失项目模块 ownership；不把清单差异直接判缺陷 |
| 历史 maps、migration records、witness | 保留 | 旧 consumer 连续迁移与来源保障 |
| DAGPipe design operators、embedded/manifest 交叉检查 | 保留 | 前者明确只供拓扑证据，后者提供资源合同保障 |
| init、pin-lock、proposal、installer、review controller | 复用 | 已有 owner 足够；新增框架会形成重复职责 |
| 全局规则正文 | 不作为 SDK payload 第二真源 | 本次改 source 和 installer；不直接改 installed Skill |

不设删减比例，不以行数判断完成，不新增规则数据库、审批账本、测试调度器或永久升级状态机。

**7．验证命令、批次与 canonical 验收**

以下命令在任务工作树运行。日志、退出码、fixture 和证据放在各自 run 目录，绑定 `aabed6a`。

开发批次按各 owner 的连贯改动运行：

```sh
cargo test --manifest-path rust/Cargo.toml --locked --test cli_smoke guidance_
cargo test --manifest-path rust/Cargo.toml --locked --test cli_smoke init_
cargo test --manifest-path rust/Cargo.toml --locked --test cli_smoke pin_lock
cargo test --manifest-path rust/Cargo.toml --locked --test sdk_0009_migration
```

新增 guard/升级用例应有明确测试名，使用 `-- --exact` 运行该用例。过滤只是开发选测；记录实际匹配数，零匹配不算验证。版本组合后受影响范围广，进入唯一 release 完整批次。

完整候选命令：

```sh
cargo fmt --manifest-path rust/Cargo.toml -- --check
cargo fmt --manifest-path dagpipe/Cargo.toml -- --check
cargo test --manifest-path rust/Cargo.toml --locked
cargo test --manifest-path dagpipe/Cargo.toml --locked --all-targets
cargo build --release --manifest-path rust/Cargo.toml --locked
bash scripts/tests/test-install-global-appsdk.sh
bash scripts/install-global-appsdk.sh
/Users/fanzhang/.cargo/bin/appsdk version
/Users/fanzhang/.cargo/bin/project-memory help
/Users/fanzhang/.cargo/bin/appsdk verify-sdk-source-registry .
```

复用现有 JSON 解析、模板/bundle 版本一致性和 bundle-layout smoke，移到适用批次；不要另造同义 validator。worktree build 的 smoke 只算 candidate 证据。

正式黑盒必须使用 `/Users/fanzhang/.cargo/bin/appsdk`。在安装前，用当前 canonical `0.1.0012` 的 `new` 生成合法旧 consumer，保存为迁移 fixture。各 fixture 指向 task-owned `APPSDK_HOME`、`COLLAB_STATE_DIR`，并设置 `GIT_CEILING_DIRECTORIES` 隔离 ancestor worktree。测试不需要 Collab 时可用不含 Collab 的 PATH，并记录可选能力缺失，不能把它称为 Collab 验收。

canonical 命令：

```sh
/Users/fanzhang/.cargo/bin/appsdk init "$consumer"
/Users/fanzhang/.cargo/bin/appsdk pin-lock "$consumer" \
  --binary /Users/fanzhang/.cargo/bin/appsdk
/Users/fanzhang/.cargo/bin/appsdk verify "$consumer"
```

在 consumer cwd：

```sh
/Users/fanzhang/.cargo/bin/appsdk guide init \
  --task guidance-upgrade --mode bootstrap --module app-core
```

每个 fixture 按上一节断言成功、拒绝和副作用。另核对三个 source/install Skill 目录相同，canonical binary 与已验 release artifact 同源。

**检查批次与触发：**开发完成一段行为后验受影响链；组合完成跑一次 release 完整批次；后续只因实质输入、环境、失败或 finding 重开受影响验证。笔记、派单、SHA 或阶段名称变化不触发全套重跑。

**8．Review、安装、集成与发布顺序**

执行中默认 L0/L1，**不安排节点级 L2 reviewer**。当前没有具体风险或证据缺口要求设计 Review。完整 milestone 提交前一次独立架构 Review 必做。

正式安装在 Review 前完成 canonical 黑盒。SDK migration Skill 当前“先 merge 再 install”的宽泛表述，应由 B 缩域：本次经授权的客户端候选安装可用于提交前验收；daemon 迁移的冻结和身份要求不能套到 SDK-only 安装。

controller 调用当前 Review 合同，milestone 使用 `profile: oauth`、`model: gpt-6.1-sol`。提供本计划、用户授权、完整候选/base/tree、审计/advisory、原始测试和安装黑盒证据。准入必须满足：

- 作者验证全部通过，候选完整。
- reviewer 与 planner/作者独立。
- exit、final JSON schema 和 controller verdict 有效。
- Review 覆盖待提交候选及整体架构。
- 历史 AGY/prose PASS、空 findings 或进程退出不能替代本次 PASS。

随后由 controller：

1. `git fetch origin`，检查最新 main 和候选组合。main 漂移只失效受影响证据；冲突停止集成。
2. 明确暂存允许文件，`git diff --cached --check`，取得覆盖该候选的有效 Review 后正式提交一次。
3. 保留既有 hook；通过正式 main/PR 流程集成。主树有未跟踪文件，不清理、不强制 checkout。
4. 核对 main tree 与已验产品候选等价，push，取得远端 SHA 和实际 CI 结果。
5. main、remote、release gate 成功后发布。若 required CI 失败，停止发布。

公开 release 以 AppSDK `v0.1.0013` 为目标，声明 macOS arm64；不要把 Linux CI 构建当作已发布平台。

最小发布资产：

- `appsdk-0.1.0013-macos-arm64`
- `project-memory-0.1.0013-macos-arm64`
- 同源三个 SDK-managed Skills 的 archive
- 上述资产的 SHA256 文件与 release notes

正式 installer 仍从对应 tag 的 source 运行，不编造已有 binary-download installer。GitHub tag source 提供完整来源；release notes 明确此次没有 Collab binary/daemon 升级。

发布前刷新 tag/release 存在性、平台和 SHA，若标签已被占用则停止。controller 在 task-owned release 目录准备资产后使用：

```sh
git tag -a v0.1.0013 <已验main-SHA> -m "AppSDK 0.1.0013"
git push origin v0.1.0013
gh release create v0.1.0013 \
  <四类已准备资产的明确路径> \
  --repo Jasonzhangf/appsdk \
  --verify-tag \
  --title "AppSDK 0.1.0013" \
  --notes-file <task-owned-release-notes.md> \
  --latest
gh release view v0.1.0013 --repo Jasonzhangf/appsdk \
  --json url,tagName,targetCommitish,isDraft,isPrerelease,assets
gh release download v0.1.0013 --repo Jasonzhangf/appsdk \
  --dir <task-owned-download-dir>
```

这些未来资产参数由 controller 从实际准备结果填入，不能提前猜测路径。下载后核对资产哈希与已验 canonical/artifact。记录 tag 实际 commit、release URL、非 draft 状态、Latest 和资产清单；仅本地 tag 或上传命令退出不算公开发布完成。

**9．资源、失败与完成判定**

共享 build 只保留一个 `rust/target` 写入 owner。B/A 不 build；C/D 需要测试时排队给 validation owner，避免 Cargo 锁等待消耗并发。DAGPipe 独立测试可并行；正式 installer、canonical 安装和 consumer 验收串行，由 controller 独占。无需新增共享锁服务。

停止与恢复边界：

- 测试、安装或黑盒失败：停 Review/发布下游，范围内修 owner，保留原错。
- 安装超时或状态未知：先查 canonical/Skills 实际状态，不能盲重放。
- Review FAIL：修 findings，补受影响验证，复核新候选。
- 冲突、push 拒绝、CI 失败：保留现场，报告 controller，不强推或绕 hook。
- 历史疑点未确认：保留 advisory，不阻断无关节点。
- 已合并变更造成真实本地回归：按交付合同新建 revert 提交恢复，不 reset main 或改写历史。
- 权限限制：只停需要该权限的动作；不得换通道绕过。当前 planner 写权限不支持安装/发布，这属于后续 controller 执行职责。

完成须分别取得以下证据：

1. 活跃范围结构审计有覆盖表、原证据、已知缺口和精确 advisory owner。
2. Source intake、模板、SDK-managed Skills 已落实审计、消融、授权复用、Guidance 可选和日常/release 触发。
3. init guard 与迁移连续性有红→绿和 canonical consumer 证据。
4. 实际 CI/hook 与文本一致；release 范围完整测试无任意 skip。
5. 正式 installer、两个 canonical binary、三个 installed Skills 同源且可用。
6. 有效独立 milestone Review、正式提交、main、remote 和 CI 各有回执。
7. `v0.1.0013` 公开 release、下载资产和 tag 来源可核对。
8. `aabed6a` 绑定交付证据后关闭；controller 完成一次结案经验审计并安全回收自有资源。

清理前先归档本计划、notes、原始必要日志、Review 与发布回执。只删除本任务创建且已无用途的 fixture、临时安装测试目录、artifact 和进程。工作树用 `git worktree remove`，dirty 不强删；保留项写 owner、原因和终点。主树既有文件、共享 base、其他任务状态和 Collab daemon 不在回收范围。

当前可以立即派 A/B/C/D。E 等接口稳定后组合，F–G 顺序推进。没有要求用户再次确认的一般问题；未知平台、发布资产路径和实时 GitHub 状态留在对应动作边界核实，不阻断当前独立实施。