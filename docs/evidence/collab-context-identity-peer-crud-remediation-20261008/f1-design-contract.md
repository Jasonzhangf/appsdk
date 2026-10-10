# F1 subagent 命令设计与失败边界合同

状态：`DESIGN_ONLY / READY_FOR_DESIGN_REVIEW`。本文件只冻结 F1 的设计图、现有调用链、
身份/授权语义和失败边界；它不证明实现、公开入口、MCP、daemon 行为或 CRUD 已完成。新设计
review 未 PASS 前，不得进入实现准入。`collab-f1-design-review-20261008-v2` 保持不可变 FAIL。

## 1. 绑定

- 任务：`collab-context-identity-peer-crud-20261008`
- 设计基线：`3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`
- 图：`docs/dagpipe/collab-subagent-command.graph.json`
- 计划：`docs/evidence/collab-context-identity-peer-crud-remediation-20261008/plan-v4.md`
- 计划接收：`docs/evidence/collab-context-identity-peer-crud-remediation-20261008/plan-v4-acceptance.md`
- Managed fixture 观察：`docs/evidence/collab-context-identity-peer-crud-remediation-20261008/o4-managed-fixture-observation.md`
- 入口证据：
  - `collab/src/main.rs:356-372`
  - `collab/src/bin/collab-mcp.rs:179-190`
  - `collab/src/bin/collab-mcp.rs:217-258`
  - `collab/src/bin/collab-mcp.rs:552-590`
- Action/wire 定义：
  - `collab/src/subagent.rs:20-77`
  - `collab/src/proto.rs:364-379`
- 既有 daemon owner：
  - `collab/src/server/mod_parts/part_11.rs:40-52`
  - `collab/src/subagent.rs:812-1155`
  - `collab/src/server/mod_parts/part_07.rs:853-1148`

## 2. 当前首个缺口

`collab subagent` 当前只把 `List` 和 `Status` 转换为 `Req::SubagentObserve`。其他 Action 除 `Start` 外直接落到 `Ok(())`，CLI 退出 0 且没有 stdout。MCP 只在子进程非 0 时设置 `isError=true`，所以空的 exit 0 被包装为 `isError=false` 和空文本。首个缺口是 CLI 分派，不是 wire 缺失，也不是 MCP 的独立协议错误。

图中五个节点表达修订后的 F1 合同：

```text
接收一次子代理命令
  -> 解析动作与输入
  -> 完成适用身份协调
  -> 核验路由与动作权限
  -> 执行动作并保留各阶段结果
  -> 输出一次完整结果
```

身份协调是独立节点。它只适用于需要 `me()` 的动作。`Start` 在解析后提前拒绝；`List`/`Status`
保持 observe 路径。图不动态选择其他分支。

图是单输入、单输出、单入口、单出口。成功、typed refusal、身份已提交但动作未执行、部分提交、
repair、unknown 和取消/清理边界都在唯一结果入口汇聚。图不画自动重试、fallback、跨 owner
rollback、身份批准工作流、peer Create/Update/Close 或第二套控制账本。

## 3. SubagentAction 矩阵

`Action` 定义在 `collab/src/subagent.rs:20-77`。下表的“当前 CLI 分支”指基线源码；“既有 owner”指 F1 应复用的真实处理函数；“结果类别”是设计要求，不是当前已实现结论。

| Action | 参数 | 当前 CLI 分支 | 既有 daemon owner / wire | 身份与授权 | 生产结果与 owner |
|---|---|---|---|---|---|
| `Start { id?, runtime? }` | 可选 child id、可选 runtime | `main.rs:357-359` 在解析后立即 `MANAGED_SUBAGENT_UNSUPPORTED` | `subagent.rs:846-855` 也保持明确拒绝 | 提前拒绝；不调用身份协调 | 非空 typed unsupported。无 child、无 managed record、无 route。owner: CLI unsupported boundary |
| `Dispatch { request_id, subject, body, feature_id?, worktree_path?, branch?, base_commit?, priority=p2, next_step? }` | 稳定 request_id、任务文本、可选 worktree/分支/基线、优先级、下一步 | 当前为空 `Ok(())` | `subagent.rs:819-845` -> `server/mod_parts/part_07.rs:853-1148`；wire 为 `Req::Subagent` | 需要 mutation principal；handler 复查 token，并要求当前 registered master authority（`part_07.rs:832-850`） | scheduler owner 提交任务、消息、reservation/admission 和可选 managed 记录；通知 owner 提交 durable message/wake。成功返回 `request_id`、`decision`、`admission`、`message_id`、`task_id`、`target` 与通知状态。通知失败可为 reservation/partial；响应丢失为 unknown，按同一 request_id 查证 |
| `List` | 无 | `main_context.rs:358-365` -> `Req::SubagentObserve { id: None }` | `subagent.rs:109-117` observe；无 mutation owner | 只要求项目 route/context；不要求 identity 协调或 mutation token | 返回结构化 `subagents`。无状态变更。owner: observe owner |
| `Status { id }` | child id | `main_context.rs:363` -> `Req::SubagentObserve { id: Some(id) }` | `subagent.rs:109-173` observe，未知 id 返回 `unknown subagent` | 只要求项目 route/context；不要求 identity 协调或 mutation token | 返回结构化记录、mailbox、任务和 keepalive 投影；未知 id 为 typed error。owner: observe owner |
| `Snapshot { id, lines=40 }` | child id、可选行数 | 当前为空 `Ok(())` | `subagent.rs:906` 明确 `SUBAGENT_SNAPSHOT_UNSUPPORTED`；`subagent_action_mutates` 会先将它当作 mutation；observe 对带 `lines` 的调用返回同一 unsupported | 走既有 wire 时，先经 mutation principal、binding 和 handler 校验，再遇明确 unsupported；只读观察路径先走项目 context 后拒绝 | 非空 typed unsupported。不读取 tmux 屏幕、不写 snapshot。owner: subagent action owner |
| `Rearm { id }` | child id | 当前为空 `Ok(())` | `subagent.rs:907-919`；父/当前 master 再认证 | mutation principal；handler 要求 parent 或当前 master | 将 keepalive 记录复位；返回 `subagent_id`、`keepalive_rearmed`、通知通道。journal 失败为 typed error。owner: keepalive owner |
| `Send { id, subject, body }` | child id、非空主题和正文 | 当前为空 `Ok(())` | `subagent.rs:1030-1089`；通过 `notify` -> `handle_send_with_task` 写入 | mutation principal；handler 要求 parent 或当前 master，并检查目标状态和任务占用 | subagent owner 可先提交 stale-working→idle；notification owner 再提交 durable message 和 wake attempt。成功返回 `subagent_id` 与 message result。通知拒绝时保留 durable msg_id、wake accepted/rejected/not-attempted/no-subscription、repair、failure、escalation；error 修复提交单独报告；后续 journal 失败不得覆盖第一次通知 `Resp` |
| `Ready { id }` | child id | 当前为空 `Ok(())` | `subagent.rs:925-1028` | mutation principal；必须由绑定 child 本人提交，`record.peer == actor` 且 thread 绑定一致（`:887-896`） | subagent owner 可提交 managed status→idle；notification owner 再投递 parent 通知。成功时保留 `subagent` 并增加 `notification` 投影。状态已提交后通知失败为 partial：同时报告 `state_commit=committed` 和通知失败。无订阅的 OK repair 必须保留。reused 不再次通知 |
| `Working { id }` | child id | 当前为空 `Ok(())` | `subagent.rs:925-1016` | mutation principal；同样要求绑定 child | 可原子提交 `TaskUpdated` 和 `SubagentUpdated`，将 assigned task 置为 working。前置失败不写状态。owner: subagent/task owner |
| `Close { id }` | child id | 当前为空 `Ok(())` | `subagent.rs:1091-1150`；父/当前 master 再认证 | mutation principal；handler 要求 parent 或当前 master | 先提交 `closing`，再提交 `closed`；返回 `closed_record_only`。不 archive/terminate runtime，不退役 registration、route、binding 或 subscription；不计 A8。owner: record-close owner |

矩阵结论：

1. 必须明确拒绝：`Start`、`Snapshot`。`Start` 在 CLI 提前拒绝；`Snapshot` 若进入 wire，由既有 handler 明确拒绝为 unsupported。
2. 必须返回结构化数据：`List`、`Status`，以及成功时的 `Dispatch`、`Rearm`、`Send`、`Ready`、`Working`、`Close`。
3. 必须保留 typed error：`Start`、`Snapshot`、未知 child、认证/scope/binding 失败、状态前置条件失败、journal/通知失败，以及需要 reservation/partial/unknown 边界的错误。
4. 没有任何 Action 允许空成功。MCP 必须从 CLI 的退出状态和真实输出同时传播结果；CLI 非 0 时
   `isError=true`，且 content 保留 stderr 的完整 `collab response: <Resp>`。CLI exit 0 但 stdout
   非法或空时，MCP 返回非空协议错误，不得得到空 content 的 `isError=false`。

### 3.1 阶段提交与响应投影

最小响应约束是现有 `Resp { ok, error, data }`：

| 场景 | CLI exit / stdout | MCP result | 必须保留的事实 |
| --- | --- | --- | --- |
| 成功 | `0`；非空、可解析 JSON | `isError=false`；非空 content | 真实 `data` |
| typed daemon failure | 非零；stderr 含原错误及完整 `collab response: <Resp>` | `isError=true`；非空 content | `ok=false`、`error`、原 `data` |
| `ok=true` + `repair_required` | `0`；非空 JSON | `isError=false`；非空 content | 完整 repair 字段；不得称 notification completed |
| 成功输出为空或非法 JSON | F1 MCP typed protocol error | `isError=true`；非空 content | 原 stdout/stderr 与缺失事实 |

`notify` 非 OK 时必须让 `client::ServerResponseError` 携带完整 `Resp`。`handle_with_env` 必须识别
并返回该完整响应；只修 `notify` 而让外层转成字符串不合格。

`Send` 与 `Ready` 的阶段事实分开：

1. durable message 是 notification owner 已提交的消息事实。
2. wake 是 transport 通知尝试，状态只能是 accepted、rejected、not-attempted 或 no-subscription。
3. consumption receipt 只能由 `recv` 产生。wake accepted 不是 consumption。
4. repair 是当前响应中的下一步事实。当前没有 durable public query 的 repair/notification
   字段必须标注为 response-only，不能声称重启后仍能公开读回。
5. 需要补充的动作事实放在 `subagent_action` 响应投影中，不建第二控制账本：

```json
{
  "subagent_action": {
    "subagent_id": "<real id>",
    "action": "ready",
    "state_commit": "committed",
    "status": "idle",
    "reused": false
  }
}
```

字段只填 owner 已确认的事实。无法确认时标 unknown，不从错误字符串推断控制状态。

### 3.2 身份协调与动作不是同一提交

`me()` 调用 daemon-owned IdentityContext。完整 facts 时，daemon 可依次提交 identity resolution、
`Req::Register`、receipt 持久化和 `Req::Context`。所以：

- 身份协调成功表示 caller 身份已协调；它不证明本次一定新建身份。
- 后续动作拒绝或未执行时，不能推理为全局无副作用。
- 必须分别记录“identity coordination committed”和“requested action not executed/refused”。
- 缺少 facts 且无可复用 anchor 时，不登记 caller；baseline/daemon 初始化另计。
- 身份协调中途失败时不提交动作；跨 owner 边界无法确认时保持 unknown。
- F1 不加只读身份 gate，也不要求用户预先执行另一登记命令。

动作错误后的身份状态通过完整错误 `data` 和现有只读诊断核对。不得从普通错误字符串推断“身份
未提交”。若需要响应直接返回更细身份阶段而现有 owner 未提供，记录为 F2 设计缺口，不在 CLI 猜造。

## 4. 调用链与身份/授权语义

F1 复用现有链路，不新增身份 gate、不做自动重试或 fallback：

```text
CLI clap Action
  -> Scope::resolve
  -> Start: immediate typed unsupported, no IdentityContext
  -> List/Status: cli_project_context + Req::SubagentObserve
  -> Dispatch/Snapshot/Rearm/Send/Ready/Working/Close: me()
       -> daemon IdentityContext may commit identity/Register/receipt/Context
       -> runtime_for_request 得到 typed RuntimeIdentity
       -> call_project -> RequestEnvelope(ProjectContext::for_registered_route)
       -> daemon validate_request_context
       -> wire route principal / mutation principal
       -> subagent::handle_with_env
       -> action handler 再认证
```

现有身份/授权层次必须分别表述，不能压成一个“统一授权 gate”：

| 层 | 事实/owner | 作用 |
|---|---|---|
| CLI 本地 | `me()`，`main.rs:47-51` -> `main_context.rs:227-250` | 调用 daemon-owned IdentityContext；完整 facts 时可提交身份登记。要求 runtime binding 有效。它不是授权决策者，也不是只读 preflight |
| 项目上下文 | `ProjectContext::for_registered_root_with_app` / `for_registered_route`，`proto.rs:284-310` | 固定 canonical root、project scope、app scope；mutation 还附带 typed runtime context |
| wire envelope | `client::call_with_runtime_identity_at_root`，`client.rs:260-268` | 把注册 route 的 runtime identity 放入请求，不在 CLI 伪造 token 或端点 |
| 项目/路由 admission | `validate_request_context`，`part_11.rs:945-1028` | 要求 project context、host route、resident project、精确 scope；未注册 route 不能成为普通 fallback |
| route principal | `wire_route_principals` / `validate_wire_route_principals`，`part_11.rs:783-938` | 核对 worker 的 route binding、project/app scope 和当前 resident identity |
| mutation principal | `wire_mutation_principal` / `validate_wire_runtime_binding`，`part_10.rs:447-621` | `Dispatch`、`Snapshot`、`Rearm`、`Send`、`Ready`、`Working`、`Close` 要求 worker/token、typed runtime context、精确 binding、非零 generation 和 binding match |
| handler 再认证 | `subagent.rs:812-904`、`part_07.rs:832-850` | 再次检查 token；Dispatch 要求 current registered master；Ready/Working 要求绑定 child；其他管理动作要求 parent 或 current master |

`me()` 进入 daemon 的 identity context owner。`server/identity_context.rs:88-147` 在 complete
facts 时 resolve identity，dispatch `Req::Register`，persist registration，再 dispatch
`Req::Context`。因此 `me()` 可能写 identity、binding、route 和 receipt。CLI 不能把本地 token
当授权真源，也不能把后续动作拒绝解释为身份未提交。`ProjectContext` 的 root/scope 是请求
路由事实，不等于授权已完成。`validate_wire_runtime_binding` 的 mutation principal 检查与
handler 的 parent/master/bound-child 检查是两层现有校验；F1 只接线，不复制、不合并、不新增
force 旁路。

`Dispatch` 是特殊路径：它在 `subagent.rs:819-845` 直接进入 scheduler owner，而不是先落入 `run()` 的普通 child 管理分支；因此它的 master 授权由 `part_07.rs:832-850` 的既有 typed grant 检查拥有。`launch_env` 当前 wire 字段在 daemon dispatch 中被忽略并以空 map 传递（`part_11.rs:41-52`）；F1 不复制进程环境，不把 launch_env 扩成新权限面。

## 5. 单一结果出口与失败边界

图只有一个结果出口。结果语义必须可区分：

| 结果类别 | 含义 | 图示/合同要求 |
|---|---|---|
| 成功 | handler 返回结构化数据，且适用持久化已提交 | 进入唯一结果出口，CLI 输出真实 JSON |
| typed refusal | 参数、动作、parent/master、bound-child、状态前置条件或 unsupported 明确失败 | typed 错误；CLI 非 0；MCP `isError=true`；不得伪造成功 |
| identity committed / action not executed | 身份协调已提交，之后 admission 或动作拒绝 | 明示身份协调可能已提交；只断言被拒绝动作的 journal、mailbox、task、wake 未提交。不能宣称全局无副作用 |
| partial | action owner 已提交部分状态，随后 notification 或后续阶段失败 | 返回各 owner 已提交事实和失败阶段；不得把 partial 描述成未执行或成功 |
| repair | `ok=true` 且 `repair_required=true`，通常为 durable message 已提交、wake 未确认或 no-subscription | CLI 保持 0、MCP 保持 `isError=false`；完整保留 repair/failure/escalation；不得称通知完成 |
| unknown | 提交响应丢失或 journal 结果无法确认 | 保留原错和可查事实，返回 unknown/下一步；不得自动重放、mint、rollback 或猜状态 |
| cancellation / cleanup boundary | 请求取消、fixture cleanup 或 runtime 清理 | 只记录对应 owner 的终点；清理不重写业务提交结果，不把人工待办当取消完成 |

具体失败边界：

- `Start`：CLI 明确拒绝；不调用 IdentityContext，无 daemon 动作副作用。
- `Snapshot`：到达既有 handler 明确拒绝，无 snapshot 写入。
- `List`：项目 route/context 失败时 typed 错误；成功时始终返回数组。
- `Status`：未知 id 返回 `unknown subagent` typed 错误；不把空对象当成功。
- `Dispatch`：request_id/主题/正文/优先级/worktree 校验失败为 action not executed；授权失败为
  typed refusal；消息/任务/admission 已提交而通知失败为 partial；响应丢失为 unknown，须按同一
  request_id 查证而非重复创建。
- `Rearm`：keepalive journal 失败为 typed 错误；成功才返回 `keepalive_rearmed=true`。
- `Send`：状态/占用前置拒绝不写 action 状态；identity 协调仍可能已提交。stale-working 修复
  或 durable message 已提交而后续失败必须说明 partial；后续 journal 失败不能覆盖第一次通知
  `Resp`；notification 响应丢失为 unknown。
- `Ready`：绑定 child 校验失败不写 action 状态；identity 协调仍可能已提交。状态提交成功但 parent
  通知失败为 partial。无订阅的 OK repair 必须保留。reused 不通知。
- `Working`：任务状态/task binding/admission 前置失败为 action not executed；提交失败为 typed error。
- `Close`：`closing` 已提交而 `closed` 失败为 unknown/partial；成功结果明确 `closed_record_only`。

本图不表达跨存储原子性。F1 只要求把现有真实结果与边界返回，不承诺多 owner 事务、跨 owner
rollback、自动补偿、自动重试或 peer CRUD 生命周期。

## 6. `close` 的准确语义

`Close` 现阶段由 `Record` 的 `closing -> closed` 两步和 `CLOSE_OUTCOME_RECORD_ONLY` 定义（`subagent.rs:97-108`、`:1091-1150`）。它只关闭 Collab 记录：

- 可以证明记录状态和回执；
- 不证明 runtime 已 archive/terminate；
- 不证明 registration、route、binding 或 subscription 已退役；
- 不证明未完成责任、进程或 worktree 已被处理；
- 不能作为 A8“关闭 peer”或完整 peer CRUD 的完成证据。

## 7. 可验证的设计风险与暂不解决项

风险与边界：

1. F1 图是合同图，节点是 design operator；`dagpipe graph validate` 和 Rust compile gate 只证明拓扑、ID、Operator 绑定和注册注册形状，不证明 handler 行为。
2. `List`/`Status` 的只读观察与 mutation 的 `me()`/binding admission 是不同路径；实现时必须保留该差异，不能为了“统一”给只读动作加 token 要求。
3. `Dispatch`、`Ready`、`Send` 都可能出现提交后失败；如果没有可公开读取的 partial/unknown 回执，必须标记 `INCOMPLETE`，不能用纯错误或纯成功覆盖。
4. `Close` 只有记录语义；完整 runtime archive、route/binding retirement、未完成责任处理和真实 peer lifecycle 另做设计，不能塞入 F1。
5. `Snapshot` 当前是 wire mutation 但 handler 明确 unsupported；F1 只需要让所有公开入口返回非空 typed 错误，不新增 snapshot 生产链。
6. `launch_env` 当前不是 CLI 输入面，daemon dispatch 也不复制它；F1 不扩张环境继承。
7. O4 已确认当前生产没有 managed create/bind 路径；完整 managed Send/Ready 成功、partial 和
   repair 黑盒依赖正式 A6 Create。普通 peer 注册和 private seed 不能替代 managed fixture。
8. `msg`、`subagent status`、`task status` 可读 durable message/task/status；`repair_required`、
   notification failure/escalation、wake attempt 目前只在响应 `data`，没有同等 durable public
   query。实现前不得把它们写成可重启读回。
9. F1 dispatcher 可以先完成实现级和公开失败路径，但不能据此宣称完整 A9/A11。A6 未闭合时，
   相关 managed happy/partial/repair 保持 `UNVERIFIED`。

明确暂不解决：

- peer Create/Update/Close 生命周期；
- 身份批准恢复和 `context --provide` 批准 wire；
- daemon/route/binding/grant/subscription 的完整退役；
- scheduler 新协议、自动重试、跨 transport fallback；
- A8/A9 的完整 live managed fixture 验收；A9/A11 的 managed 成功、partial、repair 读回。

这些项目保持 `NOT_STARTED` 或 `UNVERIFIED`，不以 F1 图校验或本文件替代。

## 8. 注册绑定与校验

图 ID 和 path 保持 `appsdk-collab-subagent-command` /
`docs/dagpipe/collab-subagent-command.graph.json`。版本从 `0.1.0` 提升到 `0.2.0`。

注册闭包：

1. `docs/dagpipe/manifest.json`：现有 ID/path 已登记。
2. `rust/src/dagpipe.rs::embedded_graph_paths()`：现有 `include_str!` 已嵌入图片节。
3. `rust/src/dagpipe.rs::design_graph_ids()`：现有 ID 已标记为 design graph。
4. `rust/src/dagpipe.rs::design_graph_operator_names()`：新增
   `appsdk.collab_subagent.reconcile_identity`，并把 inventory 数量从 72 改为 73。
5. `rust/src/dagpipe_tests.rs`：顺序测试更新为五节点。

`validate_graph_registry` 注册全部 design operators。身份协调 operator 只在设计图内可绑定。
DAGpipe 核心和 graph schema 不变。

要求验证：

```sh
dagpipe graph validate docs/dagpipe/collab-subagent-command.graph.json
cd rust && cargo test --locked --bins dagpipe -- --test-threads=1
```

验收记录必须包含命令、退出码、测试数量和完整日志路径。图通过不等于 F1 业务行为通过；实现 T2 仍需公开 CLI/MCP 红到绿证据。

## 9. 结论

本设计满足新设计 review 的候选要求：CLI 将所有 Action 明确映射到 observe 或认证 wire；`me()`
是可能提交身份协调的阶段，不与 action outcome 混同；`notify`/Ready 保留完整 `ok/error/data`
和阶段事实；MCP 不把空 exit 0 当成功。`Start`/`Snapshot` 明确拒绝；`Close` 只关闭记录。

本设计不代表产品实现完成。完整 managed Send/Ready 成功、partial 和 repair 黑盒仍依赖 A6
public Create/Bind/Route；A9/A11 未闭合。下一步是新设计 review：

```text
collab-f1-design-review-20261008-v4
```

Review 必须绑定本图/合同 hash、`dagpipe graph validate` 输出、Rust dagpipe suite 和当前候选
状态。旧 `collab-f1-design-review-20261008-v2` FAIL 不得覆盖、重试或改写。
