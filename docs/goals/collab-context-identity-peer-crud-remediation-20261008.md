# Collab context、批准身份恢复与 peer CRUD 修复任务

任务 ID：collab-context-identity-peer-crud-remediation-20261008
状态：Goal active / INCOMPLETE；AppSDK issue `86b6e8b`。**9 个节点：完成 7（T0–T5、T7）、进行中 1（T8）、未开始 1（T6）。**候选 worktree `/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008`，HEAD `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`；所有原 tracked/untracked 改动均保留。r3 milestone Review PASS（0 findings），候选绑定 HEAD 加全部 dirty/untracked 改动；回执 `.agent-collab/review/collab-context-identity-peer-crud-milestone-20261010-r3/review.final.md`。T8 已按 accepted 顺序启动：刷新 `origin/main` 为 `b76326b88baec9ea2b1b7e60c37727fc990bbe97`，旧候选落后 27 个提交，候选与上游只共同修改 6 个文件，正在准备隔离 mainline 集成。peer 与 master 必须解析到同一个 registered canonical project `main` 和 app scope；worktree 可在不同目录，路径包含关系不参与身份判定。尚未提交、集成或安装，也未改变正式 daemon/身份/peer；T6 installed/live 验收未开始。T7/T8 执行记录：`.worker-runs/collab-context-identity-peer-crud-20261008/planner-review-findings-20261010/`。
项目主树：`/Users/fanzhang/Documents/github/appsdk`。
任务依据：[四项能力审计](../evidence/collab-capability-audit-20261008/audit.md)、[公开入口观测回执](../evidence/collab-capability-audit-20261008/public-entry-observations.json)。

本文件是修复目标、验收、执行状态与证据索引的唯一任务入口。审计报告保存历史观察，不替代执行时的最新证据。独立 planner 的计划、worker 笔记、review 与回执引用到本文件，不另建竞争的目标清单。

## 1. 用户目标与完成结果

修复以下全部问题，并交付可使用的安装版 Collab：

1. 身份确认、恢复、注册由一次 `collab context` 完成。CLI/MCP 只观察和提交事实；所有身份决策、校验、更新、credential/binding/route/默认订阅持久化由 daemon 完成。无需补资料时直接返回完整状态；缺资料时准确说明缺什么、真实来源和提交方法，agent 按返回动作一次补交。
2. master 和普通 peer 都有用户批准的身份覆盖恢复路径。批准恢复不被失效的旧 credential、旧 binding 或前置 me() 卡死。用户批准的目标明确时一次提交完成后台恢复，并返回更新后的完整状态。
3. master 具备正式 peer CRUD：创建、读取、更新及关闭。创建真实可工作的 peer；关闭处理真实运行实例、注册、binding/route 及已有责任。context 按角色介绍准确命令。
4. Skill 只描述实际可执行流程。它明确 context 自动做什么、何时补资料、何时需要批准、何时执行 CRUD、如何判断成功或失败。
5. 消除审计 F1 的 CLI/MCP 写操作空成功；修复 F2/F3/F4，不以文档改称“不支持”消除本次功能要求。

完成意味着源码、公开接口、安装版、daemon、真实消费者、独立 review、Git 集成和自有资源回收均有适用证据。任务文档、单测或 review PASS 本身不等于完成。

## 2. 已知基线与未确认项

审计源码 HEAD：`2cac9e935944bddcb98fb6e7af4beb95966dcff9`。审计安装 CLI/MCP 报告 `0.2.0258`；摘要在观测回执中。源 Skill 与全局 Skill 摘要相同。以上是审计时事实，不锁定未来实现基线。

已确认断链：

| Finding | 实际行为 | 首个 owner/入口 |
|---|---|---|
| F1 | subagent 多个写动作未派发就返回 0；MCP 返回 isError=false 和空内容 | `collab/src/main.rs` 的 Cmd::Subagent；MCP call 包装 |
| F2 | context 只有四个 scalar 补交字段；无批准身份裁决；master promote 先依赖 me() | `main_context.rs`、`server/identity_context.rs`、identity resolver |
| F3 | peer 创建缺失；Present/Unknown worker close 仍依赖不支持的 snapshot；PR17 已允许确定 Missing 且无未完成任务的目标关闭，但 WorkerClosed 未退役有效 binding/route；subagent close 只关闭记录 | `subagent.rs`、server 的 worker snapshot/close owner 与 reducer |
| F4 | operations 主要是职责文字；Skill 宣传未接通路径并禁止批准补交 | server context 投影、源 Skill、MCP catalog |

主树已有 `docs/collab.md` 修改及其他任务证据，必须保留。执行前读当前 status/worktree/远端事实，记录别人的路径和资源，不暂存、提交或清理它们。本任务审计文件当前未提交；新 worktree 不会自动包含它们。只把本任务明确列出的文档/证据按原内容转入候选并记录来源，不复制主树全部 dirty 状态。

待执行时确认：

- 最新 origin/main 与审计基线差异；审计发现是否仍有效。
- 当前 canonical CLI/MCP、daemon PID/socket、实际宿主能力；源码与安装来源是否一致。
- 正式支持的宿主/transport 表，以及 peer 创建/更新/关闭的真实 API。现有文档声明和生产实现不一致时保留冲突，不能把 cfg(test) helper 当成生产能力。
- 关闭所需的真实责任条件与 runtime 所有权；snapshot 是否为必要产品契约。
- 现有幂等、事件和 rollback 能否覆盖批准恢复及 CRUD。只补已有机制无法表达的已证明缺口。

缺必需能力时独立 planner 规划最小补链；不能通过假回执、手工替代操作或删验收跳过目标。

## 3. 语义合同

### context 与补交

普通调用只有一个引导入口：`collab context`。补资料继续使用同一入口的补交能力；已有 `--provide` 可复用，不仅为“submit”名称增加第二套生命周期。

daemon 返回下列适用结果，并保持明确的 CLI/MCP 错误语义：

| 结果 | 返回内容 | agent 下一步 |
|---|---|---|
| 已登记 | 身份/角色、binding、master、peers、调度所需状态、任务、订阅与 operations | 按当前角色执行返回操作，无额外身份探测 |
| 缺资料 | 精确缺失字段、来源/获取方法、合法格式、可直接使用的提交模板 | 只提交所需真实事实 |
| 需批准裁决 | 原因、当前项目 scope、需裁决的目标/冲突、批准所需内容与提交方法 | 获取对应用户批准后提交；不猜身份 |
| 明确失败 | typed 原因、受影响节点、已提交/未提交的真实边界及准确下一步 | 只处理受影响操作，保留原错 |

格式可以复用现有 schema，但每种终点必须有可检查的语义。CLI 做参数解析和事实采集；身份事实冲突的裁定移交 daemon。秘密不进入快照、日志、消息或 Skill。

### 用户批准覆盖恢复

普通自动恢复仍沿可信当前锚点；冲突时不自动挑人。批准路径复用现有 daemon 身份 owner，不再要求先通过同一个正在失败的身份恢复算法才能提交批准。

批准必须对应具体项目和操作，并明确用户认可的目标身份或覆盖决定。任务执行授权不等于任意接管某个现有 peer/master 的实例批准；隔离验收中使用明确批准的测试身份，正式实例裁决按实际用户批准执行。

- 批准只用于指定裁决，不充当 token、端点所有权或跨项目 scope 证明。
- 核验当前调用者事实及新端点所有权；拒绝伪造端点、无批准覆盖、歧义自动选择和跨项目误接管。
- 不依赖旧 incumbent 或候选的通信 liveness 决定 master grant 是否存在。
- 身份/binding 与 master grant 保持不同控制资源和唯一 owner；恢复原 master 身份不能隐式 clear/promote。
- 必要时用户明确批准身份恢复和 master 授权替换，在同一次提交中完成适用步骤；成功返回完整快照。
- 保留原 peer 的任务、worktree、mailbox 和合法权限归属；恢复结果可读回。旧 binding 按裁决失效，不能遗留两个有效 owner。
- 丢响应后按既有幂等/读取机制确认已提交结果，不重新 mint credential 或重复创建身份掩盖失败。

具体参数和 wire 由独立 planner 提案、编排者接受并完成设计准入后冻结。不能先写猜测参数再让 Skill 迁就实现。

### peer CRUD

使用一个正式 peer 生命周期 owner；现有 worker/subagent 入口能复用则复用，重复路径经调用核对后消融。

| 操作 | 必需行为 |
|---|---|
| Create | 当前 master 创建真实宿主实例，登记独立 peer/通信 route，并返回可识别的创建状态与回执；实例实际进入可工作状态才算完成 |
| Read | context 展示 peers、角色、实例/route/责任状态和当前 master 可用操作 |
| Update | 当前 master 对允许的 peer 生命周期/绑定字段执行明确更新，保留身份及责任归属；允许字段、前置条件和不可变字段先在合同中列明 |
| Close/Delete | 检查目标与资源所有权，处理未完成责任，关闭本操作管理的真实实例并退役注册/有效 binding/route；返回可读回的终点。保留必要历史与消息证据 |

创建关闭不以记录字段代替真实宿主动作。独立 peer 的任务/worktree 不因 master 管理命令被静默删除；有责任时拒绝或走已定义的显式处理路径。非 master、跨项目或未拥有的进程/线程不能被管理命令操纵。

快照若是必要契约，接通真实生产者；若没有独立保障作用且不是关闭契约要求，明确理由后消融。不能保留永久无法满足的前置条件或写假快照。

### context 操作卡与 Skill

复用现有 operations，并按实际角色、scope 和能力给出：操作名、准确命令/参数、触发条件、权限/批准要求、前置条件、预期回执和失败后动作。master 获得 CRUD 指引，peer 获得自己的恢复/补交/协作指引。声明支持的 CLI/MCP 操作必须真正接通。

源 Skill 唯一修改位置：`collab/skills/collab/`；全局副本通过官方安装刷新。Skill 主文维护一张“状态 → 动作”表，其他章节引用；删除重复入口、退役指引和没有实现的成功宣称。help、MCP catalog、context、Skill 与当前生产能力一致。

## 4. 范围与边界

允许范围：

- `collab/src/` 中 CLI、MCP、proto、identity、server、peer/subagent 生命周期、受影响 adapter/config owner。
- `collab/tests/` 与受影响源码测试；公开入口回归和真实消费者验收。
- `collab/skills/collab/`、`docs/collab.md`、受影响的 `docs/design/` 和 `docs/dagpipe/`。
- 本文件与 `docs/evidence/collab-capability-audit-20261008/`；执行新增证据放 `docs/evidence/collab-context-identity-peer-crud-remediation-20261008/`。
- 官方 build/install 脚本仅在存在本目标必需缺口时修改。

禁止范围：

- 无关 AppSDK/DAGpipe 核心改造、Codex/DSH 产品代码、其他项目或独立 Collab 仓库。
- 直接编辑生产 identity/token/journal/routes/mailbox、reset/migrate 清状态以使测试变绿。
- 新造第二套控制注册表、通用 force 旁路、自动重试/跨 transport fallback。
- 修改全局 AGENTS 来迁就项目实现；直接修全局安装 Skill 而不修源。
- 覆盖他人 dirty 文件、claim、任务、worktree 或进程；绕过 Git hook/CI。

若必须扩大范围，先记录新的真实依赖并重新规划；不能由 worker 临时改目标。

## 5. 编排、依赖与状态

Goal 执行者是本目标编排者，负责派单、验收、集成和自有资源回收，不自动获得 Collab master 身份。

Codex Desktop/TUI 沿 codex-orchestrator；DSH 沿 dsh-create。优先已有可通信且获授权的 peer，随后使用宿主的独立 worker。未注册 Collab 不为开发自动注册、找 Master 或晋升。不能用正在失效的 subagent 写入口派发本次修复。

独立 planner、实现者、最终 reviewer 分离。Codex planner 为 fresh oauth/gpt-6.1-sol；实现 worker 默认 fresh gcm；不继承父 transcript、不 resume/fork。具体合同和启动方式引用宿主 Skill，不在本文件复制。

| 节点 | 结果/owner | 依赖 | 当前状态 |
|---|---|---|---|
| T0 最新观察 | 编排者落盘新基线、复现、能力表、资源/授权边界 | 用户启动 Goal | OBSERVED |
| T1 独立规划与设计准入 | planner 输出计划；编排者接受版本；校正图并完成适用设计 review | T0 | PLAN_V4_ACCEPTED; O5-R4 RECEIVED (A6 BLOCKED); D1-R4 VERIFIED; DESIGN REVIEW v4-r2 PASS; `plan-resume-20261009` ACCEPTED; implementation plan proposal accepted with B23-O parent refinement in `implementation-k23/plan-assessment.md` |
| T2 消除空成功 | CLI/MCP 执行 owner 修 F1，形成公开入口红→绿证据 | T1 | **COMPLETE (candidate author gate)**. Parent reviewed the exact F1 diff: Start refuses before identity coordination; List/Status use the observe route; mutations dispatch to the daemon; notification failures preserve daemon response data and committed/unknown state; MCP rejects empty or invalid JSON success output. `subagent::tests` 10/10 PASS; action routing 1/1 PASS; `cargo test --no-run --test subagent_public_entry_cli` PASS; dedicated tmux `cargo test --manifest-path collab/Cargo.toml --test subagent_public_entry_cli -- --nocapture` PASS 4/4 (isolated daemon-backed CLI/MCP). T7 remains the independent review gate for the integrated candidate. |
| T3 身份恢复 | daemon identity owner 修批准恢复与 context 补交/终点 | T1；receipt/readback 合同 | **COMPLETE (candidate author gate).** 整组 public contract 曾为 18/20，C12/C13 ignored；本轮补齐并通过 C13 restart/degraded query owner-readback 精确断言（1 passed/0 ignored），degraded query 返回原 project replay failure 且不写 journal。最终 hook-enabled `context_operation_public_contract` 20/20 passed、0 ignored；C12/C13 精确用例各 1/1 passed；no-default-features collab binary check 通过。T3 installed/live acceptance 留在 T6。证据：`b02-stable-projection-20261010/` 与 `implementation-context-cancellation-resume2-20261010/c13-owner-readback-r3.log`。 |
| T4 peer 生命周期 | peer owner 修创建/更新/关闭和宿主实际动作 | T1；D3-RUC design depends on accepted D2-A; Create also depends on A6 | **COMPLETE (candidate author gate).** R1–R4/U1–U8/C1–C9、不同目录 linked worktree 解析到同一 canonical `main` + app scope、跨 main 拒绝、重启 Close fence、Create 8/8、shared reducer/public 29/29 均已通过。隔离 daemon 的 Create/R/U/C CLI/MCP round-trip 为 `peer_lifecycle_cli` 2/2；创建调用保留 one-shot/unknown/no-resend 语义。详细 A6 准入见 `a6-create-admission-20261010/parent-acceptance.md`。 |
| T5 操作指引 | context 投影/MCP/Skill owner 对齐 T2-T4 最终合同并消融 | T2、T3、T4 | **COMPLETE (candidate author gate).** Master context 现在返回可执行 CRUD/query 操作卡，限定相同 registered canonical `main` + app scope；普通 peer 不获 peer 管理操作。MCP catalog 明确 Create 与首次调用/恢复操作 ID 的语义；源 Skill 给出触发条件、准确命令、成功回执、责任/范围拒绝和 unknown 查询步骤。操作卡通过隔离 daemon 的 master context 与 CLI/MCP round-trip 断言；operation contract 13/13 通过。官方安装后的 Skill/runtime 检查留在 T6。 |
| T6 安装与 runtime 收口 | migration owner 从 intended mainline 官方安装已审并已集成候选；完成一次 daemon 维护窗口、installed context/MCP/runtime、真实消费者和适用 blackbox；清理仅限无用途的自有资源 | T8 COMPLETE；T7 PASS 对应精确候选 | NOT_STARTED |
| T7 独立 review | 独立 reviewer 覆盖 T2–T5 完整候选、功能闭包、受影响架构与作者证据；编排者接收审查回执 | T2–T5 COMPLETE (candidate author gate) | **COMPLETE** — task `collab-context-identity-peer-crud-milestone-20261010-r3`，oauth/gpt-6.1-sol，PASS，0 findings；scope `uncommitted`, HEAD `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a` 加全部 tracked/untracked candidate 变更。正式回执 `.agent-collab/review/collab-context-identity-peer-crud-milestone-20261010-r3/review.final.md`。 |
| T8 mainline 集成与验证 | T7 PASS 后 commit、适用 PR/受保护 main 集成、CI 与远端对应回执；确认 intended mainline 内容对应已审候选 | T7 PASS | **IN_PROGRESS** — 最新 `origin/main`=`b76326b88baec9ea2b1b7e60c37727fc990bbe97`；候选基线落后 27 个提交，108 个候选路径中 6 个与上游改动路径重合。先在候选分支提交精确已审改动，再合入最新 main；隔离集成及对应验证未完成。 |

这张表定义任务依赖，不是已接受的独立实施计划。T1 可以按新证据调整实现拆分；四项总验收保持不变。共享 `main.rs`、`proto.rs` 等文件只有一个写 owner，不因表中节点不同就并发写同一文件。

当前可用增量先消除 F1 并形成准确失败/回执；随后闭合批准恢复和 peer 生命周期；最后统一操作卡并交付。依赖已满足且写范围独立的工作可并行，不为并行制造模块。

执行状态由编排者更新本表，并填写下方索引。worker 只写其独占笔记。只有目标、范围、验收、关键方案或依赖变化才重观察/重 plan。

- observation：原始 observation 在 `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/observation.md`；最新 addendum 为 `/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/latest-observation-20261009.md`，HEAD `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a` 与远端已复核一致，保留 PR #17 的 Missing worker close 修复。
- accepted plan：[plan-v4.md](../evidence/collab-context-identity-peer-crud-remediation-20261008/plan-v4.md)，接收裁定见 `plan-v4-acceptance.md`。D2/D3 v2 设计阶段接收见 [plan-d2d3-v2-acceptance.md](../evidence/collab-context-identity-peer-crud-remediation-20261008/plan-d2d3-v2-acceptance.md)，独立 planner 正文与运行证据在 `.worker-runs/collab-context-identity-peer-crud-20261008/planner-d2d3-v2/`。接受 F1 dispatch/response-preservation 可先于 A6 设计和实现；完整 managed Send/Ready public blackbox 仍依赖 A6。O5-R4：[o5-create-correlation-observation.md](../evidence/collab-context-identity-peer-crud-remediation-20261008/o5-create-correlation-observation.md) 确认 Native create response 丢失后无稳定 request-to-thread association，Collab 无可重启 public receipt query；因此 A6 BLOCKED，不允许重放或用空列表推断失败。
- 图校验 / 设计 review：D1-R4 图 `appsdk-collab-subagent-command@0.2.0` 为 5 节点/4 边，通过 `dagpipe graph validate`、13 项 DAG 测试、`cargo fmt --check` 与 `git diff --check`。旧 D1 v2 review `collab-f1-design-review-20261008-v2` 的 FAIL（2×P1）不可覆盖；v4 与 v4-r1 的 protocol failure 原始回执保留；有效 milestone review `collab-f1-design-review-20261008-v4-r2` verdict PASS。唯一 P2 为非阻断路由说明和重复校验记录，不影响设计准入，后续实现需按 Action 保持 Start/List/Status 提前结束或跳过身份协调。review 回执保留在候选 worktree 的 `.agent-collab/review/collab-f1-design-review-20261008-v4-r2/`。O4：[o4-managed-fixture-observation.md](../evidence/collab-context-identity-peer-crud-remediation-20261008/o4-managed-fixture-observation.md)。
- 候选 SHA/tree：未生成。
- 作者验证 / installed/live：未生成。
- 独立实现 review：未生成。

### 历史进度快照 — 2026-10-10 13:25 UTC

9 个节点：完成 4（T0–T3）、进行中 1（T4）、未开始 4（T5–T8）。Close 重启 fence 的唯一失败 C9 已修复并定向复测通过：`peer_lifecycle_c9_restart_preserves_unknown_close_fence_and_query` → 1 passed / 0 failed；重启后 unknown Close 可读回，Update 被 fence 拒绝且未触达宿主 settings。只复测了改动对应的 C9，没有重跑已通过的 R/U 命令。新增隔离 daemon CLI/MCP 请求 round-trip → 1 passed / 0 failed；T4 剩余门槛是 A6 Create 实现与 public acceptance；未知 thread 无稳定 lookup/cleanup，按单次派发并返回 unknown 的已接受方案处理；同一 canonical `main` + app scope、worktree 可不同目录的项目关系规则已冻结，路径包含关系不作身份依据。未安装、未操作正式 daemon/身份/peer，未做里程碑 review 或集成。
- merge / remote / cleanup receipts：未生成。
- Fresh plan：完整正文与接受裁定分别见 `plan-resume-20261009.md`、`plan-resume-20261009-acceptance.md`；SHA-256 `e8ce1907a6b63eb5fb9dea4cca1177cd6da214dec4bf6939fe2e5be3b5e3f5a4`。当前 D2/D3 没有运行中的设计 worker。下一节点 D2-A 仅映射 approved admission → 原 receipt owner → restart-safe query，不改产品/设计文件；收到并接受后再派 D2-B 与 D3-RUC。当前 dirty DAGpipe registry 属于本任务，由 parent 单一 owner G23 集成；身份与 grant 保持分离，Close 双终点和 A6 阻塞条件不变。
- D2-A 源码映射与接受裁定：[d2-admission-receipt-map-20261009.md](../evidence/collab-context-identity-peer-crud-remediation-20261008/d2-admission-receipt-map-20261009.md)、[d2-admission-receipt-map-acceptance-20261009.md](../evidence/collab-context-identity-peer-crud-remediation-20261008/d2-admission-receipt-map-acceptance-20261009.md)；map SHA-256 `d708e33e3e4573987fc09ae71280c98c7649f33499b7ddd6c764ce02f2a45918`。该证据只准入 D2-B 合同/图设计；不构成 public behavior 或产品实现通过。

### 历史进度快照 — 2026-10-10 14:33 UTC

**9 个节点：完成 6（T0–T5）、进行中 0、未开始 3（T6–T8）。**T4 完成候选作者验收：Create 8/8、R/U/C shared reducer/public 29/29、CLI/MCP CRUD isolated-daemon round-trip 2/2；Create 只派发一次，结果未知可查且不重发。T5 完成 context、MCP 与 Skill 对齐：只有 master context 收到 CRUD/query 操作卡，卡中明示同一 registered canonical `main` + app scope、cwd 范围、准确命令、成功 receipt 与失败处置。新增定向验证：master 操作卡 unit 1/1、operation contract 13/13、`peer_lifecycle_cli` 2/2、`git diff --check` 通过。Candidate test build initially exposed stale test initializers after the identity/journal fields changed; the missing test-only `invocation_ticket` and `approval_evidence` defaults are now supplied, and the affected targets compile and pass. 当前未安装或改变正式 daemon/身份/peer，T7 review 与 Git 集成都未执行。下一步是 T6 migration snapshot/freeze、官方安装、一次维护重启与 installed/live acceptance。
- 本轮 public-entry 源码：[peer_lifecycle_cli.rs](../../collab/tests/peer_lifecycle_cli.rs)，新增 master context CRUD 操作卡 assertion；宿主命令 `cargo test --manifest-path collab/Cargo.toml --locked --test peer_lifecycle_cli -- --test-threads=1` → 2 passed / 0 failed。
- context 操作卡实现位于 `collab/src/server/mod_parts/part_09.rs`，MCP 描述位于 `collab/src/bin/collab-mcp.rs`，源 Skill 操作流程位于 `collab/skills/collab/SKILL.md`。`context_gives_an_idle_master_one_canonical_scheduling_action` → 1 passed；`server::operation_contract_tests` → 13 passed。
- CLI 首次 Update/Close 省略 `--op`；该参数只用于恢复既有操作。隔离 fixture 让 Update 后出现新的 cwd command turn，因此通过实际 settings + cwd readback，而不是只看 ack。上一轮出现的 `IDENTITY_OPERATION_PROOF_MISSING` 是 fixture 错把首次操作键当重试键，已按 frozen CLI/MCP contract 修正。
- 候选 HEAD 仍为 `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`；候选含此前 dirty changes 与本轮新增 source/test/doc 修改，未提交。官方 install/live、review、merge、remote、cleanup receipts 尚未生成。

### 当前交付顺序修订 — 2026-10-10 14:40 UTC

**9 个节点：完成 6（T0–T5）、进行中 0、未开始 3（T6–T8）。**因 `appsdk-migration` §Install and restart 要求安装前先独立 Review 并集成 intended mainline，T6/T7/T8 依赖已按 fresh planner `planner-delivery-order-20261010/plan.md` 修订并接受为 **T7 Review → T8 mainline 集成 → T6 安装/live/自有资源清理**。保留 node IDs 和所有 T2–T5/A1–A12 验收；不增测试、不重做有效证据。T3、T4、T5 均为 candidate author gate COMPLETE；installed/runtime 证据仍未产生。下一步只启动 T7 一次完整 milestone Review，审查 T2–T5 完整精确候选与已有行为证据。Review PASS 后才进 T8；T8 与远端对应验证完成后才可安装或重启。
- 修订依据与 plan acceptance：`/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/planner-delivery-order-20261010/{observation.md,plan-task.md,plan.md,acceptance.md}`。
- 计划只变更交付顺序，不改功能、验收或项目 scope。此前 14:33 snapshot 的“下一步 T6”由本 amendment 明确取代，历史记录原样保留。
- B23-O audit r2 acceptance：`/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/implementation-b23-independent-audit-r2/acceptance.md`。接受范围为 B23-O 的 shared journal/replay/query prerequisite；审计发现无阻断，12 operation-contract tests exit 0。degraded query 未调用 `ProjectContext::validate()` 记为 V23/final review 非阻断关注项。审计的 `proto.rs` 与 `part_04.rs` 是 S23 修改前哈希；当前变化必须由 S23 终态 binding 和 V23 覆盖，不继承旧 PASS。
- D2-B v6 的独立 review `appsdk-d2-identity-design-review-20261009-v1` 返回 `FAIL / code_failure / blocking_findings`，列 6 条 P1：恢复 admission seam、nested Register 丢响应关联、query proof schema、批准 CLI/MCP schema 与 generation fence、query 必须只读、终态/旧 schema 一致性。原始 JSON：[review.final.md](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/.agent-collab/review/appsdk-d2-identity-design-review-20261009-v1/review.final.md)。这些是设计 findings，不是 tmux 测试阻塞。
- D2-B v7 修订 worker 按该精确 review 结果执行，仅允许修改同一四个设计文件。修订完成并取得新独立 review PASS 前，不准入产品实现；D3-RUC 继续等待，A6 仍 BLOCKED。
- 历史运行限制证据：I1 红测 `red-subagent-public-entry-cli.log` 中两个 tmux fixture 在 daemon 启动前因 socket 缺失失败；隔离 `collab context` 返回 `DAEMON_UNAVAILABLE`，daemon log 为 `path must be shorter than SUN_LEN`。当时 planner 在 `/tmp`、外置 worktree、用户目录探测 AF_UNIX `bind` 均返回 `EPERM`，loopback TCP bind 也返回 `EPERM`。该历史观测已由 2026-10-09 tmux public-entry rerun 更新：同一 `subagent_public_entry_cli` 目标通过并实际启动隔离 daemon；因此它不再构成该目标的当前阻塞。其它运行限制仍按各自新证据判断，不从旧探测外推。
- I1-R4 留下的测试目标已由 parent 接手实现；原 worker 仅完成观察和 test target 草稿，后由 parent 精确 PID 停止，记录保存在 `impl-public-entry-v4/events.jsonl`。F1 diff 已初步复核，纯逻辑 action routing 覆盖已补齐；`subagent::tests` 10/10 与 routing 1/1 当前通过。下一步是完成 changed hunks/结果语义 review；具备允许本地 socket 的执行环境后重跑 daemon-backed CLI/MCP。D2/D3 v2 仅获合同/图设计准入，不含产品实现；复用既有 O2/O5 事实，A6 保持 BLOCKED。动作级路由让 Start 解析后拒绝、List/Status 走 observe 并跳过 `me()`，其余 mutation 走现有身份/授权链。完整 managed 成功黑盒仍等 A6 解阻。I1 记录目录：`/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/impl-public-entry-v4/`。自身 worktree 为 `/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008`；原主树与他人资源保留。

## 6. 开工步骤与已有图

1. 读本文件、审计及观测回执、当前全局/适用项目 AGENTS。
2. 核对最新 origin/main、dirty/worktree、已注册的实时通信事实、canonical binaries/daemon 和必需宿主能力；落盘 observation，区分事实、判断与未知。
3. 产品代码修改和实验从最新 origin/main 创建独占外置 worktree：`/Volumes/Intel/playground/appsdk/<task-slug>`，分支使用 `codex/`。外置盘不可用就报告受影响节点，不回退本机路径。
4. observation 交 fresh 独立 planner；主 Agent 对照用户目标评估并接受完整计划，补齐实际命令/owner/allowed paths。READY 不等于设计或实现 review PASS。
5. 优先复用并校正已有语义图：`docs/dagpipe/collab-context.graph.json`、`collab-master-authority.graph.json`、`collab-pane-route-reconcile.graph.json` 及受影响订阅图。若 peer 生命周期确无现有 owner 图，只补该最小缺图并登记 manifest。
6. 图说明批准与补资料是新的 invocation，明确成功、缺事实、需批准、失败、取消和清理终点。用 dagpipe 校验；必需能力/设计边界未知时先完成适用独立设计 review，再写产品代码。
7. 每项修复先复现当前真实公开入口错误，明确第一处偏离，补必要红测后修唯一 owner。针对审计主张逐项留支持/反证，不反复重跑仍有效证据。

历史冻结合同 `docs/design/collab-master-authority-contract-20261007.md` 中 grant Empty/Assigned、scope 和批准替换语义应保留。本次批准身份恢复需更新旧 anchor/identity 文档中“只有 master 归属需要裁决”等冲突正文，不能同时执行两套规则。

## 7. 验收矩阵

每项用真实 CLI 或 MCP consumer 断言结果及副作用；测试 daemon、宿主实例与项目使用隔离状态。mock 只辅助定位，不能替代最终实际能力证据。

| ID | 公开行为 | 必需断言 |
|---|---|---|
| A1 | 可自动观察完整事实时调用 context | 一次完成注册或原身份恢复；完整快照；无额外 worker/route/init 探测；token 不泄露 |
| A2 | 无锚点/部分事实 | 精确 missing fields/source/template；无身份/credential/route 副作用；真实补交后完成；当前锚点可复用时后续不反复补交 |
| A3 | 无批准的身份冲突 | 明确拒绝并返回可执行批准路径；不误选其他 peer、不 mint 新身份掩盖旧错误 |
| A4 | peer 的批准覆盖恢复 | 用旧 credential/binding 失效样本一键恢复；目标身份与 scope 准确；不先要求失败的 me()；任务/mailbox 保留；原有效冲突绑定按裁决退役 |
| A5 | master 的批准恢复/授权替换 | 原 grant 恢复与显式替换分别验收；无隐式 promote；incumbent unknown/离线不锁死授权裁决；scope 隔离 |
| A6 | master 创建 peer | 实际宿主实例、登记、route、可工作状态可读回；创建失败/丢响应/重复提交不遗留重复实例或假成功 |
| A7 | 读取/更新 peer | context 返回准确状态与操作卡；允许更新真实生效；peer ID/任务归属不被静默改变；越权/非法字段无副作用 |
| A8 | 关闭 peer | 空闲目标真实关闭并退役有效注册/route；有未完成责任拒绝或按明确处理合同执行；不存在/越权/失败非空成功；重复关闭回执一致 |
| A9 | subagent 现有写操作 | CLI/MCP 支持项真实派发、鉴权和持久回执；未支持项明确错误；不存在 child 不再退出 0/isError=false |
| A10 | operations/help/MCP/Skill | 同一角色/状态给同一可执行流程；缺事实/需批准/失败可直接按返回方法处理；master CRUD 指引完整；无退役命令 |
| A11 | 重启与失败中断 | 原身份/合法 grant/任务/mailbox 保留；同锚点无重复；部分提交/丢响应真实可查，不重复 mint/通知/创建 |
| A12 | 既有隔离与通信回归 | 项目隔离、grant 唯一 owner、默认订阅、收件消费和受影响 transport 语义保持；没有跨 transport fallback |

宿主矩阵由 T0/T1 以真实能力冻结；验收覆盖所声明支持且受本次改变影响的宿主。无法验证的必需能力标 UNVERIFIED/INCOMPLETE，并补最小能力链，不能从文档删掉要求后宣称完成。测试不接管真实用户会话；正式恢复仅针对有实例批准的对象。

## 8. 验证与交付入口

以下命令已核对存在；在候选 worktree 根执行。planner 再按受影响测试收窄开发验证，最终完成适用 gate。测试隔离自己的 COLLAB 状态、宿主 socket/会话和进程，避免继承正式身份。

```sh
cargo fmt --manifest-path collab/Cargo.toml -- --check
cargo test --manifest-path collab/Cargo.toml --locked --all-targets -- --test-threads=1
git diff --check
dagpipe graph validate docs/dagpipe/collab-context.graph.json
dagpipe graph validate docs/dagpipe/collab-master-authority.graph.json
```

新增/修改的图逐项校验。AppSDK 消费者/注册绑定受影响时运行对应 Rust/graph gate；新开发测试与真实 consumer 的准确入口、输入和预期由 accepted plan 填实。不能把测试过滤器不存在、执行了零项、ignored 或超时当成通过。

正式安装统一使用项目根 `scripts/install-global-collab.sh`，它构建一次并安装相同候选字节、刷新嵌入 Skill。不要先独立正式 build 后再让 installer 构建第二套产物。必要单独候选 build 用 `scripts/build-collab.sh`；不绕过版本机制。

runtime 入口使用当时核实的 canonical installed binary；worktree 产物只算候选 build/test 证据。安装后按项目正式维护通道完成适用 daemon 更替；保留旧/新 PID、socket、版本、binary digest、journal/mailbox 和 context/MCP/live receipts。不把身份恢复误用成 down/up，也不自动 clear/promote 正式 master。

作者完成 T2–T5 debug、开发测试和适用候选黑盒/E2E 验收后启动一次完整 milestone 独立实现 Review，绑定完整精确候选及现有作者证据。Review PASS 后才能 commit/集成；候选合入并验证于 intended mainline 后，才能进入 T6 官方安装和 daemon 维护。installed context/MCP/runtime、真实消费者及适用 blackbox 是 T6 终点，不是 T7 前置。Review 后或集成时的实质候选变化只更新受影响验证与审查闭包。

按现行交付合同完成 commit、PR/受保护 main 集成、CI、remote 内容对应与适用 main 安装核对；不绕过 hook/保护。只回收本任务创建且已无用途的 worktree、分支、临时宿主、进程和目录；保留必要证据。dirty worktree 不强删。分层汇报 source/test/build/install/runtime/blackbox/review/merge/remote/cleanup。

## 9. 最终产物与关闭条件

必须交付：

- 四项能力的实现、正式公开接口及受影响消融。
- 已接受设计合同、图/状态机及真实 owner 映射。
- 红→绿公开入口证据、完整验收矩阵、安装字节与 live 结果。
- 源/全局 Skill 一致性及 context/help/MCP 操作卡对应证据。
- 独立 review、精确候选、CI/集成/远端和资源回收回执。
- 本文件状态表与证据索引更新；剩余未知、失败和未验证项如实记录。

只有四项总目标与全部适用终点闭合才标 DONE。局部可用增量可以单独交付，但任务仍保持未完成，继续依 accepted plan 推进剩余目标。

### 当前进度快照 — 2026-10-09 08:25 PDT

重新核对 PID 9569、候选状态与 worker 输出：S23-0-r2 仍是唯一活跃 GCM worker和唯一源码 writer；OS 进程仍在，`events.jsonl` 最新 mtime 为 08:24:xx PDT。`result.md`、`candidate-binding.json`、`worker.exit` 仍未生成，因此不能接收 S23 或派发依赖它的 V23。Goal 仍为 9 节点：3 完成、2 进行中、4 未开始；候选 HEAD `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`，36 项 dirty。

已接受的 S23-B03 可行性计划 SHA-256 `c02b0691ab100bb8e87c9984c2a5b687461709149952fbea558b716c0423bc8f` 已落实到父级文档：在 `docs/dagpipe/collab-context.graph.json` 的既有 `identity_gate` 说明中增加实现 owner/path 映射；在 D2 合同与 B23 实施合同末尾各追加版本化实现边界，保留原语义及 `partial`/`unknown`/失败合同。`jq empty`、目标文件 `git diff --check` 及 `dagpipe graph validate docs/dagpipe/collab-context.graph.json` 均 exit 0；图为 6 nodes/5 edges/6 waves。这些是文档/图映射验收，不是行为实现验收。

现阶段未启动第二个 GCM writer：S23 的 Register 与身份 adapter 源码范围有明确单 writer 约束；V23 依赖 S23 终态；T4 还依赖 B23-O/S23/V23/B23-F/VF；T5 依赖 T3、T4。不存在已接受且当前依赖齐备的第二个产品实现任务。此前未及时完成的父级文档/DAG 映射现已补齐；S23 终态接收后，按 B03 裁定补入执行索引并发出单一续派合同。

08:32 PDT recheck: PID 9569 remains live and `events.jsonl` advanced to 08:32:25. Its latest commands read `part_06.rs` Register owner; no candidate `collab/src/` file changed in the preceding 20 minutes. Current task allowlist predates the accepted S23-B03 scope delta, so this is observation only. Do not count B03 as implemented or issue V23 until S23 stops and its final binding is accepted.

## 10. 可执行 Goal

```text
/goal
你是 AppSDK 本目标的编排者，负责独立规划、派单、验收、集成与自有资源回收。
先完整读取 /Users/fanzhang/Documents/github/appsdk/docs/goals/collab-context-identity-peer-crud-remediation-20261008.md，并以该文档为任务真源。
直接执行其中的 Collab 四项修复及 F1-F4 闭环。先落盘最新 observation，再交 fresh 独立 gpt-6.1-sol planner；接受计划并完成适用设计准入后，在外置独占 worktree 开发。
按实际宿主使用 codex-orchestrator 或 dsh-create；执行 peer 优先，其次独立 worker。不要用待修复的 subagent 写入口编排本任务，也不要自动取得 Collab master。
按文档完成候选公开入口黑盒与作者验收、一次完整 milestone 独立 Review、main/远端集成验证、一次官方安装及 daemon 维护、installed/live 与适用 blackbox 验收和自有资源回收；更新任务状态与证据。只在全部适用验收完成后结束。
不要再生成一层任务或提示词，开始执行。
```
