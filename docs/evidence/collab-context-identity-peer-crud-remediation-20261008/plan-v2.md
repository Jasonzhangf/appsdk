状态：**READY——可立即派发 F1 的图修订与独立设计准入任务；通过准入后可实施 F1。完整目标仍为 INCOMPLETE；peer 创建的结果未知关联是局部 BLOCKED。**

此状态不代表图校验、设计 review、产品实现或安装验收已经通过。本轮不执行派单、写文件、产品修改、review、安装、重启或资源回收。父编排者保存本计划，评估并记录接受版本。

**1. 输入绑定**

路径约定：

```text
W = /Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008
R = /Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008
E = W/docs/evidence/collab-context-identity-peer-crud-remediation-20261008
```

| 输入 | 本轮绑定与用途 |
|---|---|
| 任务真源 | `/Users/fanzhang/Documents/github/appsdk/docs/goals/collab-context-identity-peer-crud-remediation-20261008.md` |
| 最新观察 | 完整先读 `R/observation-v2.md` |
| 原观察与旧计划 | `R/observation.md`、`R/planner/plan.md`；旧 BLOCKED 是历史判断 |
| 源码 | W；独立核对 HEAD 为 `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a` |
| O2 | `E/identity-control-observation.md`、`R/identity-capability/result.md`、任务节点笔记；源事实与参数提案分开 |
| Native 原始证据 | `R/native-owner/o1-receipt.json`、`unknown-attempt1-receipt.json`、`unknown-receipt.json` |
| 规则 | 全局 AGENTS、Plan 合同、Coding Principles、Codex Orchestrator、worker 合同、Collab Skill 与 verification reference |
| 图与注册 | context、master-authority、pane-route-reconcile、subscription、notification-consumption、notification object、manifest；`rust/src/dagpipe.rs` 与对应测试 |
| 设计与经验 | master authority 合同、anchor restore、identity minimal interaction、identity shortest path、pane ownership、daemon lifecycle；`note.md`、本目标 `run-notes.md` |

本 worktree 根未发现项目 AGENTS。Git 返回了 HEAD，但同时报告只读沙箱无法创建部分 macOS 缓存文件；不把这些警告解释为 Git 状态修改。未运行测试、graph validate 或宿主探针。

桌面 main 的 dirty 文件、其他 worktree、canonical daemon 和 O1 child 均不属于 planner 可管理资源。O1 child 未交最终结果，只能记为未确认，不能判失败或启动重叠探针。

**2. 目标与判断**

保持全部四项目标、F1–F4 和 A1–A12：

1. 一次 context 由 daemon 完成身份确认、恢复、注册及准确补交。
2. 普通 peer 和 master 均支持具体用户批准的身份覆盖恢复。
3. 当前 master 支持真实 peer 创建、读取、更新和关闭。
4. context、help、MCP、源 Skill 与安装版描述同一可执行流程。
5. 消除写操作空成功，并完成真实消费者、安装/live、独立 review、main/remote 和自有资源终点。

本轮首个可交付结果是：**CLI 将当前 subagent 动作送到既有 owner，返回实际数据或实际错误；MCP 保留该结果，不再把未执行动作包装成空成功。** 该结果可独立修复 F1，但不等于 A6–A8 完成，也不等于所有 A9 成功场景已有真实 managed 实例证据。

独立重读确认的关键事实：

| 事实 | 当前源码或原始证据 | 含义 |
|---|---|---|
| F1 首次偏离 | [main.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/main.rs:356) | Start 拒绝；List/Status 观察；其余动作直接 `Ok(())` |
| 现有 wire 已能承载动作 | [proto.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/proto.rs:368)、`part_10.rs:460`、`part_11.rs:41` | 无需为 F1 新造请求或生命周期 registry |
| MCP 成功来源 | [collab-mcp.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/bin/collab-mcp.rs:178) | 子命令 exit 0 后直接返回 stdout；根因先修 CLI |
| 批准输入不存在 | `main_context.rs:196/220`、`proto.rs:470` | 冲突先在客户端裁定；IdentityContext 只有 facts |
| stale credential 缺口 | [identity_context.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/identity_context.rs:216) | 本地 runtime 已存在就跳过 reducer credential 恢复，后续可 TOKEN_MISMATCH |
| 注册不是跨存储原子事务 | `identity_context.rs:94/125`、`part_02.rs:225`、`part_04.rs:418/515` | Register、host route、本地 receipt、Context 有独立边界 |
| PR17 合法 Missing 路径 | [part_07.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/mod_parts/part_07.rs:1317) | 无 unfinished task、确定 Missing 且有 transport 时允许无 snapshot 关闭 |
| WorkerClosed 退役不完整 | [state_impl.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/state_impl.rs:274) | 只删除 worker、keepalive、idle；不能证明 binding/route/grant/subscription 退役 |
| subagent close 是记录关闭 | `subagent.rs:1120` | 明确写 `closed_record_only`，注册和 route 仍 active |
| 普通 peer 不能直接沿用 managed 判断 | `subagent.rs:79`、`part_07.rs:387` | Record 强制 parent；按 record.peer 判 managed 会改变普通 peer 语义 |
| 正式 archive seam 未接通 | `part_01.rs:316` | AppServer 分支仍返回 tmux archive unsupported |
| 同主体 grant 延续可复用 | `part_02.rs:240–330`、`global_state_impl_part2.rs:597` | 精确旧 binding、scope、agent、generation 匹配才重签发；不得泛化 |

Native 能力只冻结到原始回执支持的范围：CLI 0.161.0，gcm/gpt-5.5/medium，真实 nonce turn 完成与读回；同 thread settings update 的 cwd 在后续实际工具执行中生效；archive 中断目标 active turn，目标不再 loaded， sibling 继续工作；第二 owned endpoint resume 同 session 并完成真实 turn。`thread/turns/list` 实际 unsupported；legacy `thread/read(includeTurns=true)` 可读历史。

这些事实不证明 Collab CRUD。初次更新后的陈旧 metadata 不推翻后续实际 cwd 与 marker 因果证据。

最新 unknown 回执比原观察更具体：断连前后查证没有 project 匹配；其中一次 loaded list 出现了 thread ID，但 `loaded_matches=[]`。因此**“空 project 列表证明没有创建”不成立**。两次尝试也不是两份安全幂等证明。

**3. 目标校正**

撤销 v1 将完整 CRUD 能力观察置于 F1 前面的依赖。F1 当前协议、入口、owner 和错误边界已足够独立设计；未知创建不应阻挡它。

保留 PR17 Missing close，不恢复“所有 close 都必须 snapshot”的旧结论。保留 no-self-close、非空 reason、unfinished task 和重复回执保护。

普通 peer 的协作身份与 runtime 资源管理权分开。master 创建普通 peer，不自动建立 `must_obey_master`；managed subagent 才建立 parent/服从关系。创建者的宿主资源管理权也不授予删除该 peer 任务/worktree 的权力。

本轮选定的最小 Update 是**同一 peer、同一 thread/session 的工作目录设置更新**。Native 已有实际生效证据。允许目录必须在该项目注册根或该 peer 已获授权的 worktree 范围内；canonical project scope 不变。endpoint 迁移不作为 A7 的首个必需 Update，以免把双 endpoint 可工作误当成旧 endpoint 已失效。

关闭采用**拒绝 busy**：unfinished task、未完成责任、待处理通知或 active turn 均拒绝，无隐式取消、转移或中断。空闲关闭仍执行目标 archive 并验证终点。Native archive 会 interrupt 的证据用于说明它是破坏性宿主动作，不能据此自动接受 busy close。

**4. DAG、SESE 与设计准入**

已注册 inventory 是 `rust/src/dagpipe.rs` 的 13 个 design graph、68 个 design operator，另有执行图。现有 manifest 没有 subagent 命令或完整 peer 生命周期图。注册设计 operator 不等于生产行为已实现。

首轮补最小 `collab-subagent-command.graph.json`，只表达 F1 的现有命令执行合同：

```mermaid
flowchart LR
    A[请求执行子代理动作] --> B[核验项目与适用调用身份]
    B --> C[执行既有动作或明确拒绝]
    C --> D[返回实际数据与错误边界]
```

四节点采用单输入、单输出 Object ARC；一个请求入口，一个结果出口。结果包含成功、明确拒绝、未执行、部分提交和结果未知。每个节点将自己的错误带到结果出口；不画补交、重试或跨 owner 回边。

拟议 operator 与映射：

| 中文语义 | 拟议 operator | 实现映射 |
|---|---|---|
| 接收动作 | `appsdk.collab_subagent.parse_request@1` | CLI Action / MCP argv |
| 核验身份 | `appsdk.collab_subagent.admit_request@1` | me、registered ProjectContext、wire mutation admission |
| 执行动作 | `appsdk.collab_subagent.apply_action@1` | `subagent::handle_with_env/run` |
| 返回边界 | `appsdk.collab_subagent.emit_result@1` | Resp、CLI out、MCP error propagation |

新增图的必要注册只触及 AppSDK 的 embedded graph、design ID/operator inventory 和 manifest；不改 DAGpipe 核心。图稿先落盘并 validate，独立设计 review PASS 后才修改这些 Rust 注册绑定与 F1 产品入口。

后续最小修订：

- context 保留六节点；identity gate 加入 observed/provided 分离、批准裁定、跨提交边界与可查结果。补交和批准是新 invocation。
- authority 保留三节点与 Empty/Assigned；修过时“待实现”描述。身份恢复只延续精确原 grant；显式 promote/clear/delegate 仍由唯一 authority owner 处理。
- pane reconcile 保留 republisher；pane 后写替换不是批准 credential 覆盖的旁路。
- subscription 保留默认 lease owner 和显式 unsubscribe。peer runtime 完整关闭增加生命周期终止的 lease 退役条件；任务完成不取消 lease。
- consumption 与 notification object 不改变身份、route 或 grant 真源。
- 完整 peer 生命周期另补六节点图：请求管理 → 核验 scope/责任/归属 → 持久化意图 → 执行宿主动作 → 提交绑定或退役 → 返回可读结果。创建分支在 unknown 关联闭合前标未准入；不以它阻挡 F1 图审查。

设计 owner 分别提交 F1 与完整生命周期审查包。F1 包必须覆盖现有 Send/Ready/Working 的持久化与通知边界，并明确记录关闭不是 runtime 关闭。

校验命令在 W 执行：

```sh
dagpipe graph validate docs/dagpipe/collab-subagent-command.graph.json
dagpipe graph validate docs/dagpipe/collab-context.graph.json
dagpipe graph validate docs/dagpipe/collab-master-authority.graph.json
dagpipe graph validate docs/dagpipe/collab-pane-route-reconcile.graph.json
dagpipe graph validate docs/dagpipe/collab-subscription-lifecycle.graph.json
dagpipe graph validate docs/dagpipe/collab-notification-consumption.graph.json
dagpipe graph validate contracts/dagpipe/notification.graph.json
```

新增完整 peer 图单独校验。注册绑定修改后在 `W/rust` 运行 `cargo test --locked --bins dagpipe -- --test-threads=1`，确认实际非零测试数、manifest 与 embedded inventory 对应及 compile gate。拓扑通过不替代设计 review 或副作用证明。

**5. 最小协议与 owner 决定**

以下新增参数均是**本计划提案**，由父接受并通过设计准入后冻结；不是当前 API。

| 对象 | 唯一 owner | 输入、围栏与完成边界 |
|---|---|---|
| F1 现有动作 | 既有 `Req::Subagent` / subagent owner | Start 保留明确拒绝；List/Status 保留 observe；其余调用既有认证 wire。`launch_env` 不复制进程完整环境 |
| context 事实 | host `ProjectRuntimeManager::identity_context` | CLI/MCP 分别提交 observed 与 provided；CLI 只做 JSON/格式检查，daemon 裁定冲突 |
| 批准恢复 | 同一 identity context owner | 目标 peer、精确 project/app scope、预期 binding/generation、批准文本、稳定 operation ID |
| binding / credential | 现有 Register transaction 与 reducer | 使用目标 authoritative credential；旧本地 token 不作批准入口前置，不 mint 新 peer 掩盖错误 |
| grant | 现有 typed grant owner | 同主体精确 generation 延续；身份恢复不隐式 promote |
| runtime 生命周期 | 现有 subagent lifecycle owner 的显式类型扩展 | 扩展 Record 为普通 peer / managed 两类；只有 managed 有 parent 与 must_obey_master |
| operation 结果 | 现有 command receipt / typed journal | 扩展必要阶段结果；不建另一账本或 registry |
| mailbox / task | 既有消息、消费和任务 owner | 保留历史与归属；生命周期命令不删除 worktree |

批准恢复继续复用 `context --provide`。提议批准项：

```json
{
  "approval": {
    "operation_id": "stable-operation-id",
    "target_peer_id": "exact-peer",
    "project_scope": "/canonical/project",
    "app_scope_id": "exact-app-scope",
    "expected_binding_id": "exact-binding",
    "expected_generation": 7,
    "text": "用户针对该实例的批准"
  }
}
```

观察事实与必要 supplied scalars 同一次调用提交。没有批准时，冲突返回 typed error、准确 scope/候选事实及批准模板；CLI 非零、MCP `isError=true`。缺事实终点保持 registered=false、无 credential/route 副作用。

批准 admission 在旧 resolver/`me()` 前处理，但仍核验 host-local socket 信任边界、canonical scope、目标当前 binding 和新 native endpoint 所有权。批准文本是用户裁定记录，不是替代 endpoint 验证的凭据。

同 operation ID、同 typed 意图优先读取已提交 receipt；相同 ID 不同输入拒绝。新 operation 的 generation 不匹配拒绝。这样允许第一次已提交但响应丢失后的原请求查证，不重复增代、签 token 或延续 grant。

Register、host route、本地 receipt、projection 分阶段记录。明确未提交才允许同意图继续；journal append/flush/reducer 结果未知保持 unknown，不能执行普通 rollback 或重新 mint。恢复查询仍走 context 同入口的 operation 查询形态；准确字段由设计包冻结，不增加另一身份恢复命令。

peer 公共入口建议扩展既有 `collab worker`，MCP 扩展对应 worker tool：

```text
worker create --request-id <stable> --kind peer|managed --runtime codex --cwd <authorized>
worker status <peer>
worker update <peer> --request-id <stable> --expected-generation <g> --cwd <authorized>
worker close <peer> --reason <text> --request-id <stable> --expected-generation <g>
```

已有 status、close 的参数保持兼容；新增字段采用明确 typed wire。所有 mutation 检查当前 master grant、精确 project/app scope、actor binding/generation、目标 binding/generation 和 runtime 所有权。

Update 不接受 peer ID、scope、任务 owner、mailbox owner、parent、管理模式或权限字段。空闲时设置 cwd；真实后续回合读取 cwd/marker 才算生效。ACK 后状态未知不能回滚猜测值；按同 thread 实际设置和 operation receipt 查证。

Close 顺序固定：

1. 校验权限、no-self-close、target generation 和该操作可管理的 runtime。
2. 拒绝 unfinished task、待处理责任、未消费通知或 active turn。
3. 记录关闭意图，防止该目标同时接受新工作。
4. 对确切 owned thread archive；不退出共享 AppServer。
5. 读回目标无 active turn、notLoaded/归档证据，并验证 sibling 仍工作。
6. 分 owner 提交 host current route retirement、项目 binding 增代 tombstone、适用 grant 撤销、目标 active lease 退役及 WorkerClosed。
7. 返回宿主与控制两个终点及 receipt；重复返回原 receipt。

复用 `pane_reclaim_events` 的事件构造原则与 route retirement helper，先核对它们各自 journal owner。不能直接把跨 owner 事件打进错误 journal。mailbox、task、审计历史保留。退役阶段失败返回“宿主已归档、控制未闭合”等准确边界，不重新 archive 或复活目标。

PR17 Missing 分支继续合法：它不虚构 archive 证据；在确定 Missing、责任清空时进行控制退役。Unknown 不是 Missing。Present/Unknown 的永久 snapshot 前置仅在真实责任与宿主终点替代保障经设计 review 通过后消融。

**6. 创建 unknown 的精确局部阻塞与最小解阻**

阻塞项是：**thread/start 发出后，thread ID 尚未持久化时，如何将宿主实例唯一关联到原创建意图，并在 adapter/daemon 崩溃后查证或结案。**

Native project/create 幂等不能解决它。request JSON-RPC ID 也不是 thread/start 幂等键。当前回执没有稳定查询证明。

安排 `O3-create-association`，先接收 O1 child 最终回执，再按未覆盖边界做一次受控实验。产品只读。最小实验：

- 为一个稳定 operation 分配独占 AppServer endpoint、home 和下游连接；同一 owner 在这个边界只允许一次 thread/start。
- 外层 consumer 丢响应时，下游连接由 adapter owner 保持，验证 Native start response 或正式 `thread/started` notification 能否携带确切 thread/session，并在**现有 typed operation journal**中持久化关联。
- 分别切断外层 consumer、adapter、AppServer：覆盖发送前、发送后未关联、关联提交后未回应三个边界。
- 重启后只读原 operation 的关联和确切宿主 API；证明一个实例可唯一恢复，或证明该 operation 的 owned runtime 已终止并达到明确取消/清理终点。
- 不能靠“这个 home 最新的 thread”、日志、prompt nonce、业务 metadata 或空 list 找对象。诊断可以保留这些内容，产品控制不能使用。
- 捕获 response 只能解外层丢响应；如果 adapter 崩溃发生在 Native 创建与关联落盘之间，仍未闭合。必须明确记录这项残余窗口。

可接受解阻证据只有：稳定原对象关联可跨该窗口查证，或 owned containment 能准确关闭该 operation 全部 runtime 并提交可验收取消终点。独占 endpoint 的提出不证明 containment 已可靠。

若两者均无法证明，A6 保持 BLOCKED，回父补 observation/replan，明确需要宿主 typed 关联能力或被批准的最小 adapter 所有权方案。**不派 create 产品实现，不重放原 unknown 请求，不清掉未核销资源。** F1、身份设计及不依赖 create 的已登记 peer Update/Close 设计继续推进。

**7. 执行任务与派单合同**

父先核实授权且可通信的 Desktop peer；不可用则 fresh 外部 GCM worker。不得用 Collab subagent writes 派发，不初始化或晋升 master。所有合同含本计划、base、目标真源、必读规则、节点笔记、允许路径、停止条件、真实命令、产物和回传目录。

| ID / worker ID | 依赖与范围 | 必交结果与接收条件 |
|---|---|---|
| D1 / `design-f1-v2` | 接受本计划；可写 F1 新图、manifest 及其设计说明，`R/design-f1-v2/`；禁止产品 Rust | F1 全动作矩阵、SESE 图、owner 映射、validate 日志、非空成功/错误边界；record-only 明确 |
| DR1 / `design-review-f1-v2` | D1；只读 | 独立 pre-implementation PASS，绑定图/合同 hash；不是实现 review |
| I1 / `impl-public-entry-v2` | DR1 PASS | F1 红→绿、现有 wire 接线、CLI/MCP consumer 与 daemon effects；未知 CRUD 不进入范围 |
| O3 / `observe-create-association-v2` | O1 child 结果接收；只读 W，只写自己的 R 子目录及 owned fixture | 上述三个中断边界、原关联查询、资源 receipt；缺一个边界不得宣称 safe create |
| D2 / `design-identity-lifecycle-v2` | O2、parent Native 原始证据；create 部分依赖 O3 | 批准输入/分阶段 receipt、Record 类型区分、Update、Close、图修订；未知 create 单列 |
| I2 / `impl-identity-v2` | D2 受影响设计 PASS；I1 交回共享入口 | A1–A5、A11 的身份部分红→绿 |
| I3 / `impl-peer-lifecycle-v2` | 对应设计 PASS；Create 另需 O3 闭合与补 plan | A6–A8 runtime 与退役；managed/普通 peer 区分 |
| I4 / `impl-operation-guidance-v2` | I2/I3 最终协议 | operations/help/MCP/源 Skill 同步；A10 |
| V1 / `author-delivery-v2` | 所有实现与作者开发黑盒 | 完整 A1–A12、installer、canonical/live、资源 receipts |
| R1 / `final-review-v2` | V1 准入 | 独立实现 review，精确候选与完整证据 |
| G1 / 父编排者 | R1 PASS、项目 gates/CI | 集成、远端、安装对应复核和自有资源核销 |

I1 唯一写范围：

```text
collab/src/main.rs
collab/src/main_context.rs（仅 F1 必要 helper）
collab/src/bin/collab-mcp.rs（仅受影响 catalog/argv/传播）
collab/tests/subagent_public_entry_cli.rs（新增真实 consumer）
必要的 collab 源码测试
rust/src/dagpipe.rs、rust/src/dagpipe_tests.rs（仅新图注册与绑定检查）
E/f1-public-entry/
R/impl-public-entry-v2/
```

I1 禁止修改 identity resolver、peer Create/Update/Close owner、proto schema、全局 Skill、安装脚本和生产状态。daemon handler 发现实际缺陷先留证据回父，不以“入口修复”扩大行为合同。

后续 `main.rs`、`main_context.rs`、`proto.rs`、MCP 文件采用串行交接给一个协议写 owner。identity 与 lifecycle 的内部文件在冻结 wire 后可并行，但 `part_02/04/06/07/10/11` 等文件逐文件分配；共享文件不得同时写。新增图登记由同一 registry owner 完成。

fresh worker 启动由父执行，`worker_id` 取表中确切值；先建立其目录与完整 `worker-task.md`：

```sh
task_cwd=/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008
worker_run=/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/impl-public-entry-v2

env -u CODEX_SESSION_ID -u CODEX_THREAD_ID \
  -u CODEX_INTERNAL_ORIGINATOR_OVERRIDE -u CODEX_APP_TOOLS_PIPE_PATH \
  codex exec --profile gcm --json --sandbox workspace-write \
  -C "$task_cwd" --add-dir "$worker_run" \
  --output-last-message "$worker_run/result.md" \
  - < "$worker_run/worker-task.md" \
  > "$worker_run/events.jsonl" 2> "$worker_run/stderr.log"
```

受限 child 需要独占可写 CODEX_HOME 时按 worker 合同建立独立 home，链接已核实的配置/auth/规则；不复制秘密，不共用 child DB。记录真实 provider/model、PID/session、退出码。禁止 resume/fork、父 transcript、跳过 repo trust。

每个 worker 在执行前和节点结束时写自己的 notes；父异常查阅先读 notes，再核实 PID/动作 receipt。没有结果不能推断未执行。worker 不 merge/push、安装或管理其他 worker；结果经 stdout/JSON 与目录回父。

**8. 可执行验证与验收**

I1 新增 `subagent_public_entry_cli` consumer，复用现有 MCP fixture 的 canonical override、stdio initialize/tools/call 和隔离 daemon 方法。它必须启动实际 CLI/MCP，不能只调用 `build_argv`。

在 W 执行：

```sh
cargo test --manifest-path collab/Cargo.toml --locked \
  --test subagent_public_entry_cli -- --test-threads=1

cargo test --manifest-path collab/Cargo.toml --locked \
  --test mcp_master_authority_cli -- --test-threads=1

cargo fmt --manifest-path collab/Cargo.toml -- --check
cargo test --manifest-path collab/Cargo.toml --locked --all-targets -- --test-threads=1
git diff --check
```

新 test target 是计划产物，创建前不能调用并宣称现有。定向执行必须非零测试数。零项、ignored、超时和中断不算 PASS。现有 mock AppServer consumer 只证 wire，不算 Native live。

F1 用例覆盖全部 Action：

- 已注册隔离 caller 对不存在 ID 的 close/rearm/send/ready/working/snapshot：CLI 非零且内容非空；MCP `isError=true` 且错误非空。
- Start 保留现有明确 unsupported，不先接触 daemon 状态。
- Dispatch 使用合法必需参数及稳定 request ID，确实到 daemon admission；当前无生产 launch 时返回真实错误，不能空成功。
- List 返回结构化数组，Status unknown 返回真实错误。
- 无身份、错误 scope、旧 generation、非 parent/master、非绑定 child 的写操作拒绝，目标 journal/mailbox/task 无副作用。
- 当前支持成功分支的持久副作用由真实 consumer 验证；没有正式可建立的 managed fixture时记 UNVERIFIED，留待 I3 后完成 A9。不得注入生产 journal或用手写 Record 冒充 live。
- `closed_record_only` 必须保留准确回执说明；它可证明记录动作，不能计入 A8。

完整总验收由下列真实场景落实：

| ID | 必需外部断言 |
|---|---|
| A1 | context 一次登记/恢复；完整快照；同身份；无 token 输出 |
| A2 | 无/部分 facts 返回精确缺失、来源、格式、模板；补交前无控制副作用；一次真实补交完成 |
| A3 | 未批准冲突拒绝；准确批准模板；不误选、不 mint |
| A4 | stale credential/binding 的普通 peer 批准恢复；任务/mailbox 保留；旧有效冲突 binding 退役 |
| A5 | 原 master 身份恢复与显式 grant 替换分开；unknown incumbent 不锁死；scope 隔离 |
| A6 | 真创建、注册、route、nonce 工作；重复/丢响应/中断按原意图查证或明确结案；无重复实例 |
| A7 | context Read 准确；cwd Update 后实际工具 cwd/marker 生效；非法字段/越权无副作用 |
| A8 | idle 真 archive与完整控制退役；busy 拒绝；不存在/越权/失败非空错误；重复 receipt 一致 |
| A9 | CLI/MCP 所有支持写项派发、鉴权、持久结果；unsupported 明确；managed 成功样本真实 |
| A10 | operations/help/catalog/Skill 同角色、scope、状态一致；模板可直接执行 |
| A11 | daemon/adapter 重启；身份、grant、task/mailbox 保留；分阶段/unknown 可查；无重复 mint/通知/create |
| A12 | 项目/app scope隔离、grant 唯一 owner、默认 lease、真实 send/recv/消费 receipt及受影响 transport 保持 |

后续新增 `identity_approval_public_cli`、`peer_lifecycle_public_cli` 两个 consumer target，按上述矩阵逐场景留 request、response、前后 typed 状态和宿主结果。Native 测试使用已选真实 provider/profile来源，禁 fallback；nonce 仅是工作证明。

canonical 复验使用同一 consumer：

```sh
COLLAB_TEST_BINARY=/Users/fanzhang/.cargo/bin/collab \
COLLAB_TEST_MCP_BINARY=/Users/fanzhang/.cargo/bin/collab-mcp \
cargo test --manifest-path collab/Cargo.toml --locked \
  --test subagent_public_entry_cli -- --test-threads=1
```

完整阶段对另外两个 target 同样运行 canonical override。fixture 应在 playground 祖先之外的自有临时项目中隔离 COLLAB_STATE_DIR、home、socket、session、tmux；W 仅承载测试代码。实际 public context 引导 fixture，不编辑正式控制状态。

**9. 并行、失败与 replan**

当前关键链：

```text
D1 → DR1 → I1 → 作者公开黑盒 → F1 增量接收
```

O3 与 D1 路径、资源独立，可并行；D2 可先完成身份和已登记 peer 合同，Create 的准入依赖 O3。容量不足优先保障 F1 图审查与实现，再 O3，不让未知创建占满验证/review容量。

停止范围以真实依赖为界：

- F1 出现既有 owner 的未知必需语义：只停该动作设计/实现，回父补观察；不伪造 unsupported 以删除长期要求。
- Create 关联窗口未闭合：仅 Create 及 A6下游停；不 replay。
- Close 不能证明真实宿主终点或控制退役：停 Close；不以 archive ACK/WorkerClosed 宣称完成。
- 批准恢复需要绕 scope/endpoint/generation：停该方案，重新规划。
- shared 文件或他人 dirty 冲突：停止相应写入，不覆盖。
- 最新 main 改变当前 owner或证据：记录双方 SHA，补受影响观察与计划；纯状态推进不重 plan。
- review FAIL、CI失败、hook失败、push拒绝：保留原错，停集成；禁止 force、skip、清状态或改断言。
- 正式 takeover 无具体实例批准：只该正式恢复停；隔离 fixture和无关主线继续。

**10. 安装、review、集成与资源**

F1 可以先完成源码与公开入口增量，完整任务保持未完成。避免每个增量都做正式安装：冻结完整候选后，使用一次官方 installer 构建并安装该候选字节：

```sh
scripts/install-global-collab.sh
```

不先单独正式 build 再让 installer 构建第二套产物。开发测试使用 debug 构建；确需独立 release candidate时走 `scripts/build-collab.sh` 并明确不冒充最终 installer产物。

项目 Collab verification reference明确规定受控 down/up维护窗口；目标授权包含正式本地维护。父 delivery owner按该项目入口执行一次维护窗口，记录旧/新PID、socket、版本、两个 binary digest、durable state 与context/MCP/live结果。它不是身份恢复方法。不得自行stop+start接管、清生产状态、promote/clear或重启非目标服务。

作者完成全部适用源码、开发测试、公开黑盒、安装/live后才进入独立最终review。planner、实现者、最终reviewer分离。review绑定候选commit/tree、图/合同hash与证据；源码/配置/测试改变后更新受影响验证和review。

父按实际项目gate、hook、CI和交付合同执行集成。MCPX若不可用，记录能力缺失并使用宿主证据，不伪造。只提交明确属于本任务的路径，不暂存桌面main dirty文件。受保护main通过正常候选/PR与CI交付，核对remote SHA和内容；不绕保护、不强推。

安装后的集成若保持已验收语义源码，核对来源和字节对应；若main组合导致实质变化，旧安装/live证据失效，修正候选后重新走适用installer与验证，不能为了“只一次”保留错误产物。

资源单逐项记录owner、路径、PID/session、socket、用途、终点。parent native-owner回执里的退出和temp删除可复用；O1 child必须由其owner交回终态，父不能代推失败或代清。共享AppServer、canonicaldaemon、其他worktree不进入worker清理清单。

必要原证据归档并接收后，确认worker停止写入，再回收自有临时home/fixture/run资源。最终普通`git worktree remove`失败于dirty则报告未收口，不force。清理缺receipt仍为INCOMPLETE。

**11. 下一轮与经验修订候选**

F1接收后补实际候选、红→绿和公开入口receipt，更新整体进度。O3新增证据闭合或改变创建方案时，再交fresh独立planner补Create实施计划。身份/Update/Close按已接受设计推进；没有依赖变化不重复计划。全部四目标及A1–A12适用终点完成后停止。

经验建议仅供父及独立reviewer复核，本planner不写Skill或memory：

| 旧结论 | 新证据与替代解释 | 建议 |
|---|---|---|
| v1需先完整能力观察再修F1 | 当前Req/Subagent、鉴权、handler及MCP链独立明确；Create unknown只影响Create | 撤销该依赖；修当轮计划，不新增全局规则 |
| 更新probe phase=FAILED | 早期metadata陈旧；后续实际cwd/marker和resume读回支持生效 | 现行能力表改为实际更新已证；保留原phase及解释 |
| 空列表可证明未create | 最新unknownloaded list出现thread但无project匹配 | 明确空查询不证明未执行；保留UNVERIFIED |
| 只有master归属需要批准 | 用户明确要求peer/master身份覆盖；源码无该入口 | 修anchor/minimal-interaction唯一正文；普通facts与批准裁定分开 |
| WorkerClosed等于关闭 | reducer只删worker；route/binding/lease残留 | 现行设计写双终点；历史审计按版本保留 |
| Desktop socket长期拒连 | 当前连接成功，Nativeowned能力已证；当前Desktop正式恢复仍未验 | 缩小note适用时点；连接不是恢复证明 |
| 通用规则需要增加未知重放禁令 | Coding Principles、DAGpipe和worker合同已覆盖 | 不更新通用Skill；补本任务操作合同与证据 |

本轮可马上推进的是 **D1、DR1和独立O3观察**；F1产品修改在自身设计准入后可推进。创建unknown继续显式局部阻塞。完整任务不降验收、不标DONE。