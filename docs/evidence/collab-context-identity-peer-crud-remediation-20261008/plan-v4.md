状态：**READY，仅指本轮依赖重排、F1 修订设计包和最小只读解阻观察可以派发。当前没有产品实现准入。完整 F1/A9 与 A6 仍有明确阻塞。**

本轮选择：**先修 F1 的真实派发、明确失败和完整响应投影；真实 peer/managed 创建作为 A6 的正式产品能力推进；创建闭合后，通过公开生命周期建立 managed fixture，完成 F1 成功黑盒。** 不再要求不存在的 fixture 先于 F1 入口修复，也不把实现级检查当作完整 A9 验收。

本 planner 只读取源文件和已有证据。没有修改文件、执行测试或图校验，没有运行 daemon、消费者或 fixture，没有创建身份/peer，没有安装、重启、review、merge 或管理他人资源。以下正文由 parent 落盘。

**1. 输入绑定**

```text
任务 = collab-context-identity-peer-crud-remediation-20261008
轮次 = dependency-replan-v4

W = /Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008
E = W/docs/evidence/collab-context-identity-peer-crud-remediation-20261008
R = /Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008
M = /Users/fanzhang/Documents/github/appsdk

已核对 HEAD = 3dfdaf8503b7a6f6a76651a1e282c038b6648c3a
```

本轮已读候选 Goal、`note.md`、`plan-v2.md`、`run-notes.md`、F1 合同、F1 rework observation、O4、原 DR1 final、原 planner-v3、现有 subagent 图及注册源码。另读了 O2、Native 能力记录、O1 child 最终报告、创建 unknown 原始回执，以及对应产品源码。

规则输入包括全局 AGENTS、完整 `coding-principals`、独立 Plan 合同、`collab`、`appsdk-migration`。DAG 判断使用 `dagpipe-runtime`，经验判断使用 `user-correction-alignment`。候选根、主树根和已检查的父目录未发现项目 `AGENTS.md`；不把模板 AGENTS 当项目规则。

候选当前 dirty 内容是已有图注册修改和任务文档/证据。没有产品实现候选。parent 必须保留各路径来源，不把已有 dirty 直接授予新 worker 所有权。

原 DR1：

```text
collab-f1-design-review-20261008-v2
结果 = FAIL / code_failure
两项 P1 = me() 可写前置协调；notify/Ready 丢失响应数据
```

该结果不可变。新设计使用新 task ID，不能覆盖或 retry 原 FAIL。

**2. 目标与独立判断**

四项能力、F1–F4 和 A1–A12 全部保持。总目标仍是安装版可用能力与完整交付，不是只修文档或错误文字。

| 分类 | 本轮判断 | 支持证据与限制 |
|---|---|---|
| 事实 | F1 首断点是 CLI mutation 未派发 | `main.rs:356` 仅拒绝 Start、派发 List/Status，其余返回 `Ok(())` |
| 事实 | MCP 将空 exit 0 包装成成功 | `collab-mcp.rs:179` 仅按进程状态判断，直接返回 stdout |
| 事实 | `me()` 会进入可写身份协调 | `main.rs:47` → `IdentityContext` → Register、receipt、Context |
| 事实 | Send/Ready 内部丢失完整通知响应 | `subagent.rs:412`、`:995`、`:1061`，以及 `handle_with_env` 的字符串错误转换 |
| 事实 | 当前生产没有 managed 创建/绑定路径 | O4 与生产 Start、Dispatch、`cfg(test)` launch helper 相符 |
| 事实 | 普通 peer fixture 不满足 managed 前置 | 普通 peer 没有 managed Record、parent、bound-child 合同 |
| 事实 | message/task/status 存在公开读回 | `msg`、`task status`、`subagent status` |
| 未知 | managed public consumer 的实际可达性 | O4 没执行 fixture；当前不存在可用创建入口 |
| 未知 | 创建响应丢失后的唯一对象关联 | 现有 Native 回执没有闭合窗口 |
| 未知 | partial/repair 完整事实的重启后公开读回 | 响应中有字段，不等于已有 durable 查询合同 |
| 假设 | 既有 typed command/receipt 机制能承载最小 lifecycle 阶段 | 必须按事件、reducer、重放和查询 owner 核实，不能从类型存在推断可用 |

O1 child 的最终 `result.md` 现已可读，状态为 BLOCKED，原因是其真实 provider nonce 回合没有 assistant 结果。parent Native owner 的成功工作观察是另一运行证据。两者不能合并成“两份成功”，也不能用 child 的环境失败证明 Native 不支持工作。parent 下一步是接收该终态及资源回执，不是重跑整套 O1。

O4 能证明**本基线的源码缺口与 fixture 分类**。它不能证明运行时通知分支已执行、创建安全性、真实消费、重启保持或 live 成功。

**3. 目标校正与方案选择**

| 方案 | 判断 | 原因 |
|---|---|---|
| 先完成真实 peer/managed 创建，再修全部 F1 | 合法，但不作为当前顺序 | A6 有未闭合的创建关联窗口；会让已确认的 CLI no-op 长期等待 |
| 先修 F1 派发/失败/结果保留，A6 后补成功黑盒 | **采用** | 可以先闭合已有公开断点，同时保留 A6→managed fixture→完整 A9 的真实依赖 |
| 私写 Record/journal 准备成功样本 | 禁止 | 不经过产品 lifecycle，不能证明 public success |
| 把普通 peer 标成 managed | 禁止 | 改变身份、权限与调度语义 |
| 将要求改称 unsupported 后结束 | 禁止 | 不能删除 A6、A9 或四项能力 |

相对旧计划：

- 保留 v3 对两个 P1 的修复方向和完整 `Resp` 传播。
- 撤销“完整 managed fixture 必须先于任何 F1 产品修改”的全局前置。
- 保留“没有 public managed fixture 就不能宣称完整 A9/F1 成功验收”。
- 解除 run-notes 中“不能通过 A6 Create 补链”的过宽表述。**本目标正式实现 A6 创建是合法主线；只禁止把测试旁路当正式创建。**
- 保留 Start/Snapshot 当前明确错误和 subagent Close 的 `closed_record_only` 边界。本轮不靠改它们名称完成 A6/A8。
- F2–F4 继续推进各自设计，不等 A6 unknown 才开始。

**4. 本轮增量与后续依赖**

本轮只细化两个 ready 包：

1. **D1-R4：F1 设计修订包。** 闭合身份前置提交、派发、完整响应、分段验证与最终 live 依赖。
2. **O5-R4：创建关联与结果读回的最小只读观察。** 查已有 schema、生产 adapter、command receipt 和原始回执；不运行宿主，不创建 fixture。

D1-R4 通过新设计审查后，才准入 I1-R4：

- I1a：CLI/MCP 真实派发及非空结果。
- I1b：Send/Ready 完整响应和已确认提交事实保留。
- I1a/I1b 可以顺序实现于一个 F1 写 owner。
- I1b 的私有状态测试只能证明实现逻辑。完整成功黑盒仍等待真实 A6 managed 创建。

后续只列必要目标：

```text
F2 身份设计 → 一次 context/补资料/批准恢复实现 → A1–A5、相关 A11/A12
创建关联解阻 → A6 创建设计 → CLI/MCP/daemon 创建实现
A6 managed 创建 → F1 managed 成功/partial/repair 黑盒 → 完整 A9
peer lifecycle 设计 → Read/Update/Close → A7/A8
最终协议 → context operations/help/MCP/源 Skill → A10
全部实现 → 作者完整黑盒/live → 独立实现 review → 集成/remote/cleanup
```

A6 的 safe-create 设计目前 **BLOCKED**。本计划不提供猜测的创建实现准入。

**5. F1 最小 wire/result 合同与唯一 owner**

请求复用现有 `Req::Subagent` 和 `Req::SubagentObserve`。本增量不新增请求 schema，不复制 launch 环境。

| 层 | 唯一责任 |
|---|---|
| CLI | 参数解析、事实采集、调用既有 daemon owner、输出结果 |
| identity owner | 选择/恢复/登记身份，credential、binding、route 和适用 lease |
| wire admission | scope、route principal、mutation principal |
| subagent owner | 动作状态、完整结果保留；Send/Ready 的动作阶段投影 |
| scheduler owner | Dispatch 的授权、reservation/admission/task/message |
| notification owner | durable 消息、wake attempt 与原始 repair/failure 响应 |
| client/CLI/MCP | 原始错误和数据的外部投影，不重新决定控制事实 |

动作矩阵保持全部十项：

- Start：当前明确非空 unsupported；不调用 `me()`。
- List/Status：保留观察路径，不新增 mutation token。
- Dispatch/Snapshot/Rearm/Send/Ready/Working/Close：真正构造现有请求并到达 daemon；真实授权和前置失败非空返回。
- Close 的记录终点不计入 A8。
- 不存在 child 的写动作不能退出 0。
- 没有任何 F1 动作允许成功空输出。

最小响应约束：

```text
保留现有 Resp:
  ok
  error
  data

成功:
  stdout = 非空、可解析的真实 JSON data
  CLI exit = 0

daemon typed failure:
  CLI exit != 0
  stderr 保留原错误及完整 "collab response: <Resp>"
  MCP isError = true，content 非空，保留该 Resp

现有 ok=true + repair_required:
  保持 CLI exit=0 / MCP isError=false
  必须完整显示 repair，不称 wake 已完成

成功空输出或非法 JSON:
  F1 MCP 返回非空协议错误
  不把防护扩成无关工具改造
```

`notify` 非 OK 时复用 `client::ServerResponseError` 携带完整 `Resp`。`handle_with_env` 必须识别并返回该完整响应。只改 `notify` 而不改外层转换仍不合格。

Send 保留现有 `subagent_id + message` 成功形状。Ready 保留现有 `subagent`，增加 `notification` 投影。失败 `Resp.data` 的原有字段保持原位置，不能搬走或裁剪。需要补充的动作事实放在一个附加 `subagent_action` 对象：

```json
{
  "subagent_action": {
    "subagent_id": "<真实 id>",
    "action": "ready",
    "state_commit": "committed",
    "status": "idle",
    "reused": false
  }
}
```

这是本次响应投影，不是新控制账本。字段只填 owner 确认的事实：

- Send 的 stale-working→idle 提交单独报告。
- Send 通知失败后，记录 error 更新的提交结果单独报告。
- 后续 journal 失败不得覆盖第一次通知 `Resp`、durable msg_id 和 repair 字段。
- Ready 状态已提交后通知失败，必须同时返回状态提交与通知失败。
- Ready 无订阅的 OK repair 数据必须保留。
- Ready reused 不再次通知；不得为补结果制造重复 wake。

身份与动作边界必须分开：

- `me()` 成功说明身份协调完成，**不说明本次一定新建了身份**。
- 之后动作拒绝时，不能称全局无副作用。
- 缺事实且无可复用锚点时，不登记 caller；baseline/daemon 初始化另计。
- 身份协调中途失败时不提交动作；无法确认的跨 owner 边界保持 unknown。
- F1 不新增只读身份 gate，也不要求用户预先执行另一登记命令。

D1 必须冻结外部说明：动作错误后的身份状态通过现有完整错误数据及只读诊断核对；不得从普通错误字符串推断“身份未提交”。如果要求响应直接返回更细身份阶段，而现有 owner 没提供该事实，记录缺口交 F2 设计，不在 CLI 猜造。

**6. A6 的最小宿主/transport 设计约束**

正式创建使用一个 lifecycle owner，复用 `collab worker` 和相应 MCP surface。建议公共形状保持 v2 提案，但目前只是设计候选：

```text
collab worker create --request-id <stable> --kind peer|managed --runtime codex --cwd <authorized>
collab worker status <peer>
```

`peer` 与 `managed` 是显式创建意图：

- 普通 peer 独立身份与责任，不能因 master 创建就被强制 managed。
- managed 创建必须由正式 lifecycle 提交真实 parent、child、thread/binding 和 managed Record。
- 不通过事后改 flag 将普通 peer 转为 managed。
- 两种模式复用同一创建流程和宿主动作；只在权限、parent、调度合同上区分。

最小所需能力：

| 能力 | 当前证据 | 准入要求 |
|---|---|---|
| 选定 owned endpoint、真实 thread/start/read | Native 观察有证据 | 精确版本/schema、endpoint owner、cwd/session/thread 校验 |
| 真实工作回合 | parent owner 有运行观察；child 未通过 | public create 后实际工作与结果读回 |
| 更新 cwd | Native 后续实际工具回合支持 | 不能只看即时 metadata 或 settings ACK |
| archive 确切 thread、保留 sibling | Native 观察支持 | lifecycle 的责任检查、双终点和控制退役 |
| thread/start 幂等 | 未证明，schema 无 thread 级键 | 不把 JSON-RPC ID 或 project 幂等键当 thread 幂等 |
| 发送后、关联落盘前的恢复 | **未闭合** | 稳定关联可查，或被证明的 owned containment 取消终点 |
| Collab operation receipt | 有 typed 基础，覆盖不完整 | 核实阶段持久化、重放、查询与同意图复用 |

创建状态至少区分：

```text
已接收意图
→ 宿主请求尚未发送
→ 宿主请求已发送但关联未知
→ 宿主对象已关联
→ 登记/绑定/route 已提交
→ 工作能力已验证
→ 创建完成
```

另有明确失败、部分提交、取消已闭合、清理未闭合终点。`已发送但关联未知` 不能自动回到 start。

幂等处理：

- stable request ID 绑定 actor、scope、kind、runtime、cwd 等 typed 意图。
- 同 ID 同意图查原结果；同 ID 不同意图拒绝。
- 外层响应丢失但对象已关联时，查同 operation，不再 start。
- append/flush/reducer 结果未知时，停止依赖写入。
- 只有明确证明原动作未执行，才允许按冻结合同继续。
- 部分登记失败不能再创建一个对象替代旧对象。

退役与责任：

- lifecycle owner 负责自己创建的 runtime，并记录确切宿主 owner。
- Close 拒绝未完成任务、待处理责任和不允许关闭的活动状态。
- 宿主归档与控制退役分别验收。
- route、binding、适用 grant、active lease、worker registration 各由原 owner 退役。
- 保留 task、mailbox、审计历史和 worktree。
- 不退出共享 AppServer，不删除其他 peer，不以 `WorkerClosed` 代替全部退役。
- PR17 确定 Missing 分支保留；Unknown 不当 Missing。

创建解阻观察只接受两类证据：

1. 原意图与确切 Native 对象在关键窗口后仍可通过 typed 控制关系查证。
2. 独占 containment 能确切终止该意图全部 runtime，并提交可验收取消/清理终点。

“空列表”“最新 thread”“日志 nonce”“业务 metadata”都不能代替该关系。仅捕获正常 response 也不能证明 adapter 崩溃窗口已闭合。

**7. DAG 校正与注册闭合**

F1 业务合同图保持单源单汇，增加独立身份协调阶段。它仍是 design graph，不冒充可执行 runtime graph。

```mermaid
flowchart LR
    A["接收一次子代理命令"] --> B["解析动作与输入"]
    B --> C["完成适用身份协调"]
    C --> D["核验路由与动作权限"]
    D --> E["执行动作并保留各阶段结果"]
    E --> F["输出一次完整结果"]
```

唯一结果包含成功、拒绝、身份已协调但动作未执行、partial、repair、unknown、取消边界。下游收到终态时只传递结果，不执行副作用。Start 提前拒绝、List/Status 不需身份协调，必须在动作路由表中明确；不动态改图，不新增 fallback。

| 业务节点 | Operator/实现映射 | 必需说明 |
|---|---|---|
| 解析动作 | `appsdk.collab_subagent.parse_request@1` | Start 提前拒绝；参数错误无动作 |
| 身份协调 | 新 `appsdk.collab_subagent.reconcile_identity@1` | `me()` 调用 daemon，可写；观察动作不协调 |
| 路由与授权 | `appsdk.collab_subagent.admit_request@1` | route/mutation/handler 权限分别保持 |
| 动作与阶段结果 | `appsdk.collab_subagent.apply_action@1` | Dispatch、状态提交、通知委托与完整 Resp；引用既有通知/消费图 |
| 完整输出 | `appsdk.collab_subagent.emit_result@1` | CLI/MCP 非空结果与真实边界 |

最小需改：

```text
E/f1-design-contract.md
docs/dagpipe/collab-subagent-command.graph.json
rust/src/dagpipe.rs
rust/src/dagpipe_tests.rs
```

注册核对：

- 保持图 ID 和 path；提升设计图版本以标识新字节。
- `manifest.json` 已有 ID/path；只在现有条目确需版本/描述更新时编辑。
- `embedded_graph_paths()` 已有 `include_str!`；无需重复新增。
- `design_graph_ids()` 已含 ID；无需改数量。
- `design_graph_operator_names()` 新增身份协调 operator；更新实际数量。
- 更新节点顺序测试，继续验证 manifest/embedded/registry 闭合。
- 不改 DAGpipe 核心、不加未支持 schema 字段。

旧证据已核读：4 节点、3 边、4 waves；相关 Rust suite 13 passed、0 failed、0 ignored。后续另一 binary 的 0 tests 不是注册验证证据。以上只绑定旧图。

D1-R4 未来执行：

```sh
cd /Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008

dagpipe graph validate docs/dagpipe/collab-subagent-command.graph.json
cargo test --manifest-path rust/Cargo.toml --locked --bins dagpipe -- --test-threads=1
cargo fmt --manifest-path rust/Cargo.toml -- --check
git diff --check
shasum -a 256 docs/dagpipe/collab-subagent-command.graph.json
shasum -a 256 docs/evidence/collab-context-identity-peer-crud-remediation-20261008/f1-design-contract.md
```

新设计 review task 建议：

```text
collab-f1-design-review-20261008-v4
```

context 身份图、master-authority 图、pane-route-reconcile 图及必要 lifecycle 图在后续对应设计包更新。创建图只能形成标明缺能力的草案；关键关联窗口未解阻时不能批准为创建实现图。

rollback/cleanup 合同：

- F1 不承诺跨 owner rollback。
- durable 消息和已提交状态不因通知失败回滚。
- unknown 不自动重放。
- fixture cleanup 是实际 fixture owner 的外层生命周期，不能改写业务终点。
- 创建取消必须有宿主和控制终点；告警或人工待办不算取消完成。

**8. 执行任务、owner 与唯一写路径**

| ID | 关联验收/解阻 | 依赖 | Owner 与可写范围 | 必交结果 |
|---|---|---|---|---|
| P0 接收计划 | 全目标依赖 | 本正文 | parent；Goal、run-notes、note、计划正文 | accepted version、取舍、路径 ownership |
| D1-R4 | F1/A9、相关 A11/A12 | P0 | 单一 design/registry owner；第 7 项四类文件及 `R/design-f1-v4/` | P1 对照、动作矩阵、五节点图、分段验证、hash/日志 |
| O5-R4 | A6、完整 A9/A11 | P0 | 只读 owner；仅自己的 `R/observe-create-contract-v4/` | exact schema/关联/receipt/查询能力表；不能形成虚假 READY |
| DR1-R4 | F1 实现准入 | D1-R4 | 独立 reviewer，只读 | 新 task 的设计结论，绑定新图/合同 |
| I1-R4 | F1 入口与结果实现 | DR1-R4 PASS | 单一 F1 implementation owner；下列 allowlist | 红→绿、公开失败/观察成功、实现级响应测试、剩余 live 依赖 |
| D2 | F2/A1–A5 | O2、现有原始证据 | 后续身份 design owner；相应合同与图 | 批准入口、阶段 receipt、同入口查询；另行冻结 |
| D3 | F3/A6–A8 | O5；Create 另需关联解阻 | 后续 lifecycle design owner | 普通/managed 区分、创建与退役合同 |
| I2/I3 | F2/F3 | 对应设计准入、共享文件交回 | 后续独占实现 owner | 真实 context/CRUD 黑盒 |
| I4 | F4/A10 | 最终协议 | guidance owner；源 Skill、operations、help/catalog | 一张状态→动作表、可执行模板 |
| V/R/G | 全集 | 全部实现和适用验收 | 作者验证 owner / 独立 reviewer / parent delivery owner | live、review、集成、remote、cleanup |

I1-R4 allowlist：

```text
collab/src/main.rs
collab/src/main_context.rs              仅 F1 必要 helper
collab/src/subagent.rs
collab/src/bin/collab-mcp.rs            仅 F1 argv/result/catalog
collab/src/main_tests.rs               仅受影响测试
collab/src/main_tests_part2.rs          仅受影响测试
collab/src/subagent_tests.rs
collab/tests/subagent_public_entry_cli.rs  新 target
R/impl-public-entry-v4/
```

I1-R4 禁止：

```text
proto.rs、client.rs
identity resolver、server/identity_context.rs
server/mod_parts/part_02/04/05/07/10/11.rs
peer lifecycle/adapters
安装脚本、全局 Skill/AGENTS
生产 identity/token/journal/routes/mailbox
```

若既有通知字段或公开查询不足，I1 记录具体缺口回 parent。不得扩大 allowlist 或在 CLI 重建控制事实。

O5-R4 的完整只读命令基础：

```sh
cd /Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008

rg -n 'thread/start|thread/started|idempotency|thread/list|thread/resume' \
  /Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/native-capability/schema \
  /Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/native-owner

rg -n 'CommandStarted|CommandCompleted|CommandReceipt|operation_id|command_receipts' \
  collab/src/proto.rs collab/src/server

rg -n 'repair_required|notification_delivery|consumed_by_recv|SubagentUpdated|WorkerClosed' \
  collab/src
```

O5 只检查相关定义与原始响应，不输出凭据。必须回答：

- Native 是否有稳定意图→thread 查询关系；证据绑定哪个版本。
- `thread/started` 能否关联确切 request，断连/重启后是否仍可查；只有 schema 声明时标未验证。
- 现有 command receipt 能否保存阶段与未完成 operation，而非仅 completed receipt。
- 哪个公开 owner 可以按原 operation 查询 partial/unknown。
- repair/wake 事实哪些 durable、哪些仅响应可见。
- 最小剩余运行观察是什么；不能直接运行旧 probe。

各 worker 只写自己的 notes/result。parent 独占更新 Goal、`run-notes.md`、`note.md` 和 E 的汇总证据。共享 `main.rs`、`main_context.rs`、`proto.rs`、MCP 和 registry 文件按 owner 串行交回。

parent 派 fresh worker 时使用完整 task 文件，禁止父 transcript、resume/fork。不使用待修 subagent 入口派发本任务：

```sh
task_cwd=/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008
worker_run=/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/impl-public-entry-v4

env -u CODEX_SESSION_ID -u CODEX_THREAD_ID \
  -u CODEX_INTERNAL_ORIGINATOR_OVERRIDE -u CODEX_APP_TOOLS_PIPE_PATH \
  codex exec --profile gcm --ephemeral --json --sandbox workspace-write \
  -C "$task_cwd" --add-dir "$worker_run" \
  --output-last-message "$worker_run/result.md" \
  - < "$worker_run/worker-task.md" \
  > "$worker_run/events.jsonl" 2> "$worker_run/stderr.log"
```

这是未来 parent 操作。planner 不执行。若需要独占 child home，按宿主合同建立，不复制秘密或共享 DB。记录实际路由、session、退出码。

**9. implementation-level checks 与黑盒验收**

新 `subagent_public_entry_cli` 必须启动真实 CLI 和 MCP stdio。复用既有隔离环境及外部 AppServer RPC fixture。新 target 创建前，命令不是已有执行证据。

I1a 当前可以完成的公开断言：

| 用例 | 输入与外部断言 |
|---|---|
| B1 完整事实的新 caller | Dispatch 因非 master 拒绝；caller 身份协调完成；grant 不变；动作 message/task/admission 未创建 |
| B2 缺事实且无可复用锚点 | 完整 missing projection；无 caller credential/binding/route/lease；动作未执行 |
| B3 不存在 child | Send/Ready/Working/Rearm/Close/Snapshot 非零、非空；MCP `isError=true` |
| B4 Start | 提前明确拒绝，不调用身份协调 |
| B5 List/Status | List 非空结构化数组；unknown Status 明确错误；保留观察权限 |
| B6 无 managed child 的合法 master Dispatch | 到达真实 daemon admission，返回真实 unavailable/unsupported；不能空成功 |

示例公开输入：

```sh
collab subagent dispatch --request-id f1-denied-v4 --subject F1 "fixture task"
collab subagent send missing-v4 --subject F1 "fixture task"
collab subagent ready missing-v4
collab subagent working missing-v4
collab subagent rearm missing-v4
collab subagent close missing-v4
collab subagent snapshot missing-v4
collab subagent list
collab subagent status missing-v4
```

这些命令由隔离 consumer 设置真实 cwd、caller 和 binary；不得在生产项目执行。

I1b 在 public fixture 暂不可用时允许的检查：

- `cfg(test)` 私有状态可用于 notify、typed error、Ready repair、后续 journal failure 的实现级红→绿。
- 明确标为 implementation-level。
- 证明原始 `Resp.error/data` 不丢失，后续错误不覆盖 durable 字段。
- 证明 Ready reused 不重复通知。
- 不能记为 A6、managed public success、Native live 或完整 A9 PASS。
- 不能以这些检查启动覆盖完整 managed 行为的最终架构 review。

A6 正式 managed 创建后，必须补：

| 最终 F1 用例 | 必需外部断言 |
|---|---|
| Send 成功 | public create 得到真实 managed child；CLI/MCP 派发；message/task 可查；真实消费 |
| Send durable 后 wake rejected | 非零/`isError=true`；msg_id、repair、failure、escalation 保留；未假称消费；无第二消息 |
| Ready 状态提交后 wake rejected | idle 可查；通知 durable；完整错误；不撤销状态 |
| Ready 无订阅 | owner 用公开 unsubscribe；OK repair 完整；没有 wake；recv 后才有消费 receipt |
| Ready reused | 不产生第二消息/wake |
| Accepted/Rejected/NotAttempted | 保留不同语义；外部 RPC fixture 可证协议分支，Native live 另证真实提交/消费 |
| 重启/丢响应 | 按确切 operation/message/status 查证；不重 mint、重 create 或重 wake |
| journal 故障 | 实现级注入证明错误保留；不新增生产 fault API，不冒充 live 可达 |

未来作者验证命令：

```sh
cd /Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008

cargo test --manifest-path collab/Cargo.toml --locked \
  --test subagent_public_entry_cli -- --test-threads=1

cargo test --manifest-path collab/Cargo.toml --locked \
  --test mcp_master_authority_cli -- --test-threads=1

cargo fmt --manifest-path collab/Cargo.toml -- --check
cargo test --manifest-path collab/Cargo.toml --locked --all-targets -- --test-threads=1
git diff --check
```

每次证据绑定 source/tree、图/合同 hash、binary digest、fixture 配置、真实请求/响应、前后公开状态、退出码、非零测试数和资源终点。零项、ignored、超时、中断不计 PASS。

**10. A1–A12 全集与接续安排**

| ID | 保持的最终验收 | 依赖/当前状态 |
|---|---|---|
| A1 | 一次 context 自动登记或恢复，完整快照，无额外探测，无 token 泄露 | F2；未完成 |
| A2 | 精确 missing/source/template；补交前无身份副作用；一次真实补交完成，后续不反复补 | F2；F1 只补既有边界检查 |
| A3 | 未批准冲突明确拒绝，提供可执行批准路径；不误选、不 mint | F2 批准设计；未完成 |
| A4 | stale credential/binding 的 peer 一键批准恢复；保留任务/mailbox；退役旧有效冲突 binding | F2；未完成 |
| A5 | master 原 grant 恢复与显式替换分别验收；无隐式 promote；unknown incumbent 不锁死；scope 隔离 | F2 + 既有 grant owner；未完成 |
| A6 | 真宿主实例、登记、route、工作能力；重复/失败/丢响应无重复实例或假成功 | 创建关联 BLOCKED；先 O5 |
| A7 | context Read 准确；允许 Update 实际生效；身份/责任不变；非法/越权无副作用 | lifecycle 设计；未完成 |
| A8 | 空闲目标真实关闭、控制退役；责任拒绝；不存在/越权/失败明确；重复 receipt 一致 | lifecycle 设计；record-only 不计 |
| A9 | 所有支持写项真实派发、鉴权、持久回执；unsupported 明确；managed 成功真实 | F1 分段推进；完整成功依赖 A6 managed |
| A10 | operations/help/MCP/Skill 同角色状态给同一可执行流程；CRUD 完整 | F2/F3 最终协议后统一 |
| A11 | 重启保留身份/grant/task/mailbox；partial/丢响应可查；无重复 mint/wake/create | 贯穿各包；未完成 |
| A12 | 项目/app scope、grant 唯一 owner、默认订阅、消费和 transport 保持，无 fallback | 各包回归与最终 live |

F2 最小后续设计保留：

- 普通事实与批准裁定分开，均由 daemon 处理。
- 批准入口在失败的旧 resolver/`me()` 前接收。
- 仍核验 exact scope、目标 binding/generation、新 endpoint 所有权。
- 同 operation 同意图查原 receipt；冲突意图拒绝。
- identity/binding 与 master grant 分 owner。
- 外层 context 的阶段查询沿同一入口设计；不新增恢复命令。
- 不把任务授权当正式实例 takeover 批准。

F3 Update 首轮只设计已观察支持的 cwd 更新；冻结允许字段、不可变字段和真实生效证明。Close 先闭合责任与宿主/控制双终点。不要扩展权限编辑或通用进程管理。

F4 由源 `collab/skills/collab/` 维护一张状态→动作表。批准字段、CRUD 命令和回执必须等协议冻结后写。全局副本仅通过官方安装刷新。

**11. 并行安排与分阶段提交边界**

当前 ready 层是 **D1-R4 与 O5-R4**。二者可以并行，路径独立。DR1-R4 等 D1；I1 等新设计 PASS。

资源不足时：

1. 保证 D1 和新设计审查容量。
2. 用剩余容量做 O5。
3. F1 implementation 后，优先推进创建解阻与 F2 设计。
4. 不让未知创建实验或非关键文档占满验证/review 容量。

当前完整关键链：

```text
D1 → 新设计 PASS → I1 入口/结果实现
O5 → 必要关联观察 → 创建设计 PASS → A6 managed 创建
两链汇合 → F1 managed 黑盒 → F4 对齐 → 完整 live → 实现 review → 集成 → cleanup
```

F2 和既有 peer Update/Close 的设计可与创建观察并行。共享产品文件只允许串行 writer。不要为并行拆出重复协议层。

建议 patch/commit 边界：

| 边界 | 内容 | 可提交条件 |
|---|---|---|
| C0 | F1 图/合同/注册修订 | 新设计验证与对应设计结论；不称产品完成 |
| C1 | F1 CLI/MCP 派发与响应保留 | 先作为工作中候选；适用公开黑盒/live 未齐前不提交为已交付修复 |
| C2 | F2 context/批准恢复 | 对应作者黑盒/live、独立实现 review 后 |
| C3 | F3 创建/更新/关闭及必要公开结果查询 | 创建关联闭合、真实 lifecycle 黑盒/live、独立 review 后 |
| C4 | 最终 operations/help/MCP/Skill 与完整回归 | A1–A12 全集核对、最终精确候选 review 后 |

**推荐先保持 C1 为未集成候选，完成 A6 后验证完整 managed 行为，再提交经 review 的 F1 单元。** 如果 parent 要先独立交付 C1 的入口子增量，必须在新设计审查中冻结该范围，并完成其适用安装/live 与独立实现 review；任务状态仍为 INCOMPLETE。不得仅凭私有单测提交“完整 F1”。

本计划没有授权 planner 提交。所有 Git 动作归 parent delivery owner。

**12. review、安装、集成与 cleanup 闭合**

顺序保持：

```text
实现
→ 作者 debug/开发测试
→ 公开黑盒
→ 适用官方安装与 controlled live
→ 独立实现 review
→ commit/受保护 main 集成/CI
→ remote 内容与安装来源核对
→ 自有资源 cleanup
```

新设计 review 与最终实现 review 分开。planner 不兼任 reviewer。原 DR1 FAIL 保留。

正式候选安装由 delivery owner 使用：

```sh
scripts/install-global-collab.sh
```

它构建一次并安装相同字节。不先另做正式 release build 再安装第二套版本。安装前核实 canonical 路径与现行 runtime。按 Collab 本项目 verification 入口执行一次受控维护窗口；本 planner 不执行。

安装后复验同一 public consumer：

```sh
COLLAB_TEST_BINARY=/Users/fanzhang/.cargo/bin/collab \
COLLAB_TEST_MCP_BINARY=/Users/fanzhang/.cargo/bin/collab-mcp \
cargo test --manifest-path collab/Cargo.toml --locked \
  --test subagent_public_entry_cli -- --test-threads=1
```

后续新增 identity/lifecycle consumer 在对应计划中冻结命令；不能提前把未创建 target 记为已验证。

live 必须分别证明：

- 新 binary/version/digest 与候选对应。
- daemon PID/socket 与维护窗口对应。
- 身份、grant、task/mailbox 保留。
- public create 得到真实 peer/managed child。
- 真实工作、通知、recv 和 durable consumption。
- Update 实际 cwd 生效。
- Close 宿主与控制终点，以及 sibling 不受影响。
- MCP initialize/catalog/result 与安装 CLI 一致。
- 源/安装 Skill 字节一致。

最终 review 绑定精确候选、图/合同和证据。review 后源码/配置/测试变化，重做受影响验证与 review。

parent 集成前 fetch 最新 main，组合差异并判断哪些证据失效。只暂存本任务明确路径。受保护 main 走正常 PR/CI，不 bypass hook、不 force。MCPX gate 能力可用时使用；不可用就保留能力缺口与宿主证据，不伪造。

资源表由实际 owner 记录：path、PID/session、socket、用途、责任终点和 receipt。只回收本任务创建且已无用途的资源。O1 child 的终态由 parent 接收，不能代其清理。dirty worktree 不强删；共享 AppServer、生产 daemon、主树 dirty 和其他 worktree 不进入 cleanup 清单。

**13. 失败、停止与下一轮条件**

只停止受影响依赖链：

- 新设计没有通过：停 I1，不停 O5/F2 只读设计。
- `Resp` 任一层仍丢字段：停 F1 结果接收。
- 必需提交事实没有公开读回：该 A9/A11 分支 INCOMPLETE；记录 owner 缺口。
- 创建关联窗口未闭合：停 Create 及 managed 成功黑盒，不重放 start。
- Close 只有 archive ACK 或 WorkerClosed：停 A8 接收。
- 正式身份覆盖缺实例批准：停该正式恢复，隔离测试仍可按测试批准推进。
- shared writer/dirty ownership 不清：停相应写入，不回退他人内容。
- main/candidate/环境改变：仅失效受影响证据，补 observation/replan。
- review、hook、CI、push 失败：保留原错，停止集成，不改断言或门禁。

下一轮触发：

1. D1-R4 与新设计结论齐备：按已接受范围推进 I1，不重复规划普通状态推进。
2. O5 明确剩余关联窗口：只为该窗口形成最小观察合同；需要运行实验时由 parent 按真实资源与权限派发，本轮不执行。
3. 关联能力闭合或方案改变：交独立 planner 冻结 A6 创建实施包。
4. F2/F3 最终协议冻结：再细化 F4 操作卡和 Skill。
5. 全部 A1–A12 与交付终点闭合后停止，不再制造下一轮。

**14. 经验修订候选**

| 旧结论 | 新证据、反证与独立性 | 更新建议 |
|---|---|---|
| F1 仅接线就能保留全部结果 | DR1 与直接源码一致；均来自同一源码，不算两次运行证明 | 修任务合同和 allowlist，加入 subagent 结果 owner |
| me 是只读 preflight | IdentityContext/Register/receipt 调用链反证 | 删除该描述，按身份/动作阶段验收 |
| F1 实现必须等完整 managed fixture | O4 证明当前无 public 创建；入口失败黑盒可独立执行 | 缩小到完整 managed 成功验收依赖；允许入口/投影分段实现 |
| 不得通过 A6 创建解除 fixture 缺口 | 本轮用户明确允许正式 A6 创建；private seed 仍无效 | 修改任务计划边界，允许正式产品能力补链 |
| project 幂等或空列表能解决 thread 创建 unknown | schema/原始 unknown 回执反证；loaded thread 与空 project match 并存 | 保留 A6 BLOCKED，不推断未执行 |
| O1 child 尚无终态 | 当前最终报告可读，nonce 工作未通过 | parent 接收其 BLOCKED 与资源回执；不与 controller 成功混计 |

这些是项目计划/设计修订候选。建议 owner 在独立复核后修唯一正文。全局 AGENTS 和通用 Skills 已覆盖真实结果、unknown、唯一 owner 和分段依赖，本轮没有新增全局规则或长期 memory 的必要。未执行的新设计行为保持 UNVERIFIED。