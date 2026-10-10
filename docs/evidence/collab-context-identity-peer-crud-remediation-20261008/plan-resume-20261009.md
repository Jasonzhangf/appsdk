**状态：READY，仅适用于下面的 D2/D3 设计执行与设计准入计划。D2/F2、D3/F3 产品实现尚未准入；A6 Create 保持 BLOCKED；总 Goal 保持 active / INCOMPLETE。**

**当前真正可执行的下一节点是 D2-A：交付“批准恢复 admission → 原 receipt owner → 同入口 query”的有限设计切片。** 先由 parent 保存并接受本计划，再用 fresh GCM worker 执行。D2-A 必须先交出可读产物，不能再次把完整源码巡视和全部设计写作放进一个长时间无产出的任务。随后完成 D2 合同与图，D3 复用已冻结的 receipt 接口，分别进入独立设计 review。

本轮只读取规则、计划、任务笔记、图、源码和原证据。没有修改文件、创建 worker/worktree、执行产品命令、测试、图校验、网络或 daemon 动作。没有执行 identity/task/peer 写操作、安装、重启、review、Git 集成或资源回收。完整正文由 parent 接收后保存。

**1. 输入绑定与证据范围**

以下缩写在本计划中只表示精确路径：

```text
W = /Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008
R = /Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008
E = W/docs/evidence/collab-context-identity-peer-crud-remediation-20261008
M = /Users/fanzhang/Documents/github/appsdk

任务 ID：
collab-context-identity-peer-crud-remediation-20261008

最新 observation：
R/latest-observation-20261009.md
时间：2026-10-09 05:13 UTC

绑定候选 HEAD：
3dfdaf8503b7a6f6a76651a1e282c038b6648c3a

主树 observation 基线：
2cac9e935944bddcb98fb6e7af4beb95966dcff9
```

本轮已完整读取最新 observation、v2 planner 正文、v2 接收裁定、v4 计划、任务真源、F1 checkpoint、O2/O5 观察、相关 run notes、旧 D2/D3 合同与笔记，以及 D1 实际 review final。`plan-f2-f3-v1-acceptance.md` 不在候选指定位置，本轮读取了其真实位置：

```text
M/docs/evidence/collab-context-identity-peer-crud-remediation-20261008/
  plan-f2-f3-v1-acceptance.md
```

后续派单必须使用这个实际路径，或由 parent 先将本任务文件按原内容转入候选并记录来源。不能继续传播不存在的候选路径。

有效原证据继续引用：

- `E/f1-implementation-checkpoint-20261008.md`。
- `E/identity-control-observation.md`、`R/identity-capability/result.md`。
- `E/o5-create-correlation-observation.md`、`R/observe-create-contract-v4/`。
- `R/native-owner/unknown-attempt1-receipt.json`、`unknown-receipt.json`、`native-owner-runtime-receipt.json`。
- `W/.agent-collab/review/collab-f1-design-review-20261008-v4-r2/` 的 final、status、meta、events、exit。
- `E/run-notes.md`、`R/design-d2-identity/notes.md`、`R/design-d3-peer-lifecycle/notes.md`。

planner 的实际 `oauth/gpt-6.1-sol` 路由、fresh 启动和退出码应由 parent 的本次启动记录证明。本正文不以身份自述代替该证明。

**2. 目标、事实、假设与未知**

四项能力、F1–F4、A1–A12，以及 installed/live、独立实现 review、Git 集成和资源收口全部保留。本轮推进设计阻塞链，不能把设计交付算作修复完成。

| 分类 | 判断 | 对计划的影响 |
|---|---|---|
| 事实 | D1-R4 有有效设计 review PASS，范围仅为 D1/F1 | 保留，不重做，不授予 D2/D3 准入 |
| 事实 | F1 有局部实现与测试证据；完整 daemon-backed consumer 未验证 | 保留 dirty 候选，不称完整 F1/A9 PASS |
| 事实 | `identity_context` 在一般 route admission 前接收请求 | D2 批准恢复应在这里进入 |
| 事实 | 外层 `IdentityContext` 无阶段 receipt；nested Register 有自己的 receipt | 必须设计安全的 bootstrap admission 和原 owner 的阶段/query 扩展 |
| 事实 | `CommandStarted` reducer 不保存可查询阶段；completed receipt 不能覆盖外层 unknown | B23 是未实现基础依赖 |
| 事实 | `WorkerClosed` 不等于宿主停止或完整控制退役 | D3 必须保留双终点 |
| 事实 | 原 D2/D3 已中断；无 result、无设计变更；最新 observation 确认无旧进程 | 可以安排替代节点；不能接收旧任务为设计成果 |
| 事实 | 现有 peer 一个 lost，一个 endpoint live 但 agent unknown；无可验证 dispatch 通道 | 当前选 GCM worker，不自动注册或晋升 |
| 事实 | O5 未闭合 Create 关联窗口，也没有 restart-safe public receipt query | A6 保持 BLOCKED |
| 假设 | 原 typed event/receipt owner 可最小扩展承载外层阶段与查询 | D2-A 必须给出 producer/reducer/replay/query 的具体映射或精确缺口 |
| 未知 | 当前 daemon PID/socket、候选与安装字节等价性 | 当前设计不依赖；留到 runtime 动作边界核实 |
| 未知 | 完整责任 writer 集合及各 host adapter 的停止/query 保证 | D3 只补这些未覆盖源码切片 |
| 未知 | 两个旧 worker 为什么停止产出 | 不能从 0% CPU 推断模型、provider 或产品根因 |

F1 的 AF_UNIX/loopback `EPERM` 是已记录的验证环境限制。它既不是产品 PASS，也不是产品失败。本轮不再探测，不换 transport，不用 mock 替代。

**3. 对旧计划的裁定**

| 旧计划内容 | 本轮裁定 |
|---|---|
| v4 的 F1 dispatch、完整 `Resp`、身份与动作提交分离 | 保留；以当前 checkpoint 更新执行状态 |
| v4 的“D1/O5 为当前 ready 层” | 过时；两者已有结果 |
| v2 的 D2 唯一 identity owner、批准入口、scope/ownership、identity/grant 分离 | 保留 |
| v2 的 D3 Read/Update(cwd)/Close、责任 fence、宿主/控制双终点 | 保留 |
| v2 的 D2/D3 worker IN_PROGRESS | 撤销；改为 interrupted/no deliverable |
| v2 中广泛 O2-B/O5-U 再观察 | 收窄；复用有效 O2/O5，只读取明确未覆盖的符号或新能力证据 |
| registry 等外部 owner 移交 | 已过时；接收裁定和 run notes 已确认 parent 为 G23 唯一 owner |
| D2/D3 全包同时派发 | 调整为先交 D2-A，再完成 D2；D3 使用同一接口合同，避免各造 receipt 语义 |
| 旧合同只修 anchor 冲突正文 | 不足；`collab-identity-minimal-interaction.md` 仍明确禁止 approval/manual selection，D2 必须同步修该权威冲突 |
| D3 图包含可实施 Create 流程 | 不接受；本轮图只准入 R/U/C，Create 仅列阻塞依赖 |
| 全仓 `cargo fmt --check` 作为无条件新改动门禁 | 不能掩盖 checkpoint 已记录的 baseline 格式差异；记录基线，检查本次 changed files，最终 gate 如实报告 |
| 局部测试 → 最终实现 review/集成 | 不接受；公开黑盒及适用 installed/live 仍是准入条件 |

没有撤销任何用户能力。需要消融的是重复身份判断、冲突规则、无独立保障的 snapshot 前置和重复生命周期实现，不是验收要求。

**4. DAG 与基础依赖**

复用现有项目图及注册：

```text
docs/dagpipe/collab-context.graph.json
docs/dagpipe/collab-master-authority.graph.json
docs/dagpipe/collab-pane-route-reconcile.graph.json
docs/dagpipe/collab-subscription-lifecycle.graph.json
docs/dagpipe/collab-subagent-command.graph.json
docs/dagpipe/collab-notification-consumption.graph.json

docs/dagpipe/manifest.json
rust/src/dagpipe.rs
rust/src/dagpipe_tests.rs
```

D1 图与 review 保持有效。master-authority 图的 Empty/Assigned、精确 scope、原 grant owner 保持。D2 不新建 authority owner。

本轮执行依赖为：

```text
最新 T0 observation
→ parent 接受 fresh plan
→ D2-A admission/receipt/query 切片
→ D2-B 完整合同与既有图修订
→ G2 注册与静态校验
→ DR2 独立设计 review

D2-A 接口合同
→ D3-RUC 合同与生命周期图
→ G3 注册与静态校验
→ DR3 独立设计 review

DR2/DR3 对应 PASS
→ 获准基础实施包
→ 基础契约单测
→ identity/lifecycle 核心
→ CLI/MCP/context 组合
→ 公开黑盒与 installed/live
→ 独立实现 review
→ Git/remote/cleanup
```

A6 是另一条仍阻塞的必需链：

```text
宿主关联或 owned cancellation 能力证明
→ restart-safe public query 合同
→ fresh Create 规划与独立设计准入
→ public peer/managed Create
→ 完整 F1 managed 行为、完整 CRUD 指引
→ A1–A12 总交付
```

D2 图表示一次 invocation 到一次结果。普通调用、事实补交、批准提交、原 operation query 是互斥请求意图。补交和批准是新的 invocation；query 不续写副作用。

D2 必须区分：缺事实、需批准、拒绝、已准入、部分提交、结果未知、完成、提交前取消。未知不能自动回到 Register。

D3 图表示一次 R/U/C 请求到一次结果。Read/query 不生成 mutation intent；Update/Close 才进入持久意图。结果包括拒绝、partial、unknown、complete、cancelled、cleanup-open。每张项目图保持 SESE，业务节点使用中文语义，源码映射另列。

**5. 本轮任务合同**

下列路径均相对 W，除明确标出的 R 路径。所有 worker 都必须知道：不是 worktree 唯一使用者；保留 D1/F1、任务证据和其他 dirty；不撤销他人修改。

| ID | 唯一 owner、依赖、验收作用 | 精确 allowed paths | 必交产物 |
|---|---|---|---|
| P0 | parent；依赖本正文；完成 T0→plan→接受 | `R/planner-d2d3-v3/`；本任务 Goal、`E/run-notes.md`、`W/note.md` 的当前任务汇总 | 完整 plan、acceptance、执行节点/owner/停止条件；修正仍称 worker running 的过时状态 |
| D2-A | fresh GCM design worker；依赖 P0；解阻 A3–A5/A11 | 仅 `R/design-d2-admission-v3/` | `admission-receipt-map.md`、notes、result；无产品/设计正文写入 |
| D2-B | fresh GCM D2 design worker；依赖 D2-A 被接收 | 下列五个 D2 文件；`R/design-d2-contract-v3/` | 完整合同、冲突正文修订、两张图、operator map、逐例验收和 hashes |
| D3-RUC | fresh GCM lifecycle design worker；依赖 D2-A 接口；解阻 A7/A8 | 两个新 D3 文件；`R/design-d3-ruc-v3/` | R/U/C 合同、图、责任 writer/fence 表、adapter/query 表、验收与 hashes |
| G2/G3 | parent；接收对应设计包后 | `docs/dagpipe/manifest.json`、`rust/src/dagpipe.rs`、`rust/src/dagpipe_tests.rs` | 在保留 D1 增量基础上完成必要注册、非零验证、冻结 review 输入 |
| DR2/DR3 | 不同于 planner/设计作者的独立 reviewer；对应 G 完成 | 产品/设计只读；各自 review evidence | 绑定精确合同、图、registry 和来源的独立设计裁决 |

D2-B allowed files：

```text
docs/design/collab-identity-context-recovery-contract-20261008.md
docs/design/collab-anchor-restore-model.md
docs/design/collab-identity-minimal-interaction.md
docs/dagpipe/collab-context.graph.json
docs/dagpipe/collab-pane-route-reconcile.graph.json
```

旧正文只修改与本次批准恢复、daemon 事实裁定、阶段/query 及新合同引用直接冲突的内容。保留自动 anchor recovery、token 保密、现有 scope 与 transport 校验。

D3-RUC allowed files：

```text
docs/design/collab-peer-lifecycle-contract-20261008.md
docs/dagpipe/collab-peer-lifecycle.graph.json
```

subscription 图先只读。若 D3 证明图本身必须修改，交 parent 记录精确差异及唯一 writer，再扩该单文件合同。不能提前授予广泛订阅改造。

所有本轮 design worker 禁止：

```text
collab/src/**
collab/tests/**
registry 三文件
D1/F1 合同与候选实现
docs/collab.md
collab/skills/collab/**
全局 AGENTS/Skill
主树全部写入
生产或隔离 identity/token/journal/routes/mailbox 写入
产品命令、host 创建/更新/关闭、daemon/network 探测
git add/commit/merge/push、安装/重启、资源清理
```

D2-A 只能读与缺口有关的源码；不重做完整 O2：

```sh
cd /Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008

rg -n 'IdentityContext|CommandEnvelope|actor_binding_id' \
  collab/src/proto.rs collab/src/server/identity_context.rs \
  collab/src/server/mod_parts/part_04.rs

rg -n 'CommandStarted|CommandCompleted|command_receipts|commit_command_locked' \
  collab/src/server/mod_parts/part_02.rs \
  collab/src/server/state.rs collab/src/server/state_impl.rs \
  collab/src/server/global_state_models.rs
```

产物必须回答：

1. 无有效旧 binding 时，host-local identity owner 如何核验恢复意图。
2. 核验后的内部 intent 如何进入原 receipt owner，不能伪造 `actor_binding_id`。
3. 哪些阶段当前没有 producer/reducer/replay/query。
4. operation 标识如何在副作用前可供调用者保留，并绑定精确 scope、target、action、规范化意图。
5. query 如何在旧 credential 失效时仍安全核验归属。
6. 当前可复用基础与必须新增基础逐项分开；基础不存在可以设计最小扩展，但不能声明已具备。

D2-A 结果为 `READY_FOR_CONTRACT` 或 `BLOCKED`。`READY_FOR_CONTRACT` 只允许继续 D2-B，不授予产品实施。

D3 未覆盖源码观察限于责任写入、停止/query 与退役：

```sh
rg -n 'WorkerClosed|worker_closures|unfinished|admission_frozen|archive_thread' \
  collab/src/server/mod_parts/part_07.rs \
  collab/src/server/mod_parts/part_06.rs \
  collab/src/server/state.rs collab/src/server/state_impl.rs \
  collab/src/adapters

rg -n 'assignment|claim|pending|closing|retire_runtime_binding|pane_reclaim_events' \
  collab/src/server/mod_parts/part_01.rs \
  collab/src/server/mod_parts/part_07.rs \
  collab/src/server/mod_parts/part_08.rs \
  collab/src/server/mod_parts/part_09.rs \
  collab/src/server/mod_parts/part_10.rs
```

命中后读取实际定义与 caller；搜索结果本身不算 owner 证明。

**6. D2 必须冻结的合同**

唯一身份编排 owner 保持 `ProjectRuntimeManager::identity_context`。CLI/MCP 只做语法、真实事实采集、请求与结果投影。

批准恢复在旧 resolver/`me()`/binding failure 前进入，但不能削弱普通 authenticated mutation 的校验。必须保留：

- canonical project、实际 app scope 和唯一目标 identity。
- 新 endpoint/session/thread 的真实 adapter ownership 校验。
- 明确用户批准、被覆盖的旧冲突条件和 generation。
- observed/supplied facts 分开；冲突由 daemon 裁定。
- 权威 credential 恢复；本地 stale receipt 不能成为真源。
- 不因 `TOKEN_MISMATCH` mint 新 identity。
- identity、binding、route、本地 credential 保存、grant、lease 各自提交边界。
- 同 operation 同意图复用；同 ID 不同意图拒绝。
- partial/unknown 查询、daemon replay 后同入口 readback；不自动续写。
- 完整 snapshot 和非秘密阶段结果；公开 query 不返回 token。

恢复 master identity 和替换 master grant 是不同意图、不同 owner 事务。原同主体 grant reissue 复用 `part_02` 的既有匹配条件。显式替换调用原 authority owner；普通恢复不能 promote/clear。

D2-B 必须给出准确 CLI argv、MCP `tools/call` JSON、typed 请求/结果 schema、query 权限，以及可执行验收命令文件。字段尚未冻结时不能把候选语法放进产品操作卡。

**7. D3 必须冻结的合同**

唯一 daemon lifecycle owner 复用 `part_07.rs`；宿主动作委托真实 adapter；binding、route、grant、lease 由各原 owner 退役。managed Close 最终委托共同 lifecycle owner，不能在 `subagent.rs` 再造一套。

Read 无 mutation。context/status/query 同源，分开 ordinary/managed、Missing/Unknown/Closed/CleanupOpen。普通 peer 不暴露 managed 私有 assignment。

Update 首轮只允许 `cwd`。必须冻结允许目录和责任冲突。identity、token、scope、transport、thread/session、binding/generation、grant、parent/kind、task/mailbox/worktree 归属不可静默改变。settings ACK 不等于生效；精确目标后续真实执行必须证明新 cwd，再核对 public projection。宿主已生效但控制失败返回 partial，不重建目标。

Close 必须具有：

1. 精确 target、scope、binding generation、SelectedTransport 和 ownership。
2. 责任检查与 close intent 的同一 admission fence。
3. 每个相关责任 writer 对 fence 的检查位置。不能只在 Close 入口查一次。
4. 宿主 RPC 在全局 state lock 外执行；回来后复核 operation/generation。
5. 精确宿主停止或适用归档、活动 turn 终止证据。
6. worker、有效 binding、route、适用 lease/subscription、合法权限退役证据。
7. task、mailbox、history、worktree 和 receipt 保留。
8. 未知结果可 query；不自动重复 archive/close。

责任拒绝须列实际责任 ID 和已有处理动作。历史 mailbox 不要求清空。当前 master 不可自 Close。普通 peer 的独立责任与 managed 的 parent/assignment 责任分别检查。

PR17 的确定 Missing 边界保留，但补控制退役。Unknown 不转 Missing。旧 record-only `WorkerClosed` receipt 不能自动升级为完整 Close；设计必须说明兼容读回如何区分历史记录终点与完整 lifecycle 终点。

snapshot 若无独立保障且不是用户关闭合同要求，消融该前置；不能保存永久 unsupported 门槛或生成假 snapshot。

**8. A6 保持阻塞的处理**

本轮不派重复 O5-U，不写 Create 命令、实现或可实施图。现有证据已足够维持 BLOCKED。

只有新证据满足以下之一，才能触发 A6 解阻规划：

| 路径 | 必须同时满足 |
|---|---|
| 稳定关联 | typed 原 request/intent → 精确 thread；覆盖 response loss、关联保存前崩溃和 restart；公开查询可恢复；重复提交不产生第二实例 |
| owned cancellation | 发送前已持久且独占的 ownership handle；能确切取消该意图全部目标；持久残留处理有证明；durable cleanup public query 经 restart 仍可查 |

空列表、JSON-RPC ID、response 后保存 thread ID、project 幂等键、nonce、最新 thread、停止 endpoint PID 均不足。

parent 保持一个明确的待办：接收宿主/launcher 对上述能力的**新版本接口与原始证明**。没有新能力输入时不重复 start 实验。出现能力变化后补窄 observation，再交 fresh planner；不能修改 Codex/DSH 产品来绕过本任务禁止范围。

B23 的 Collab receipt 改造本身不能修复 Native sent-but-unassociated 窗口。

**9. 执行入口、进度与旧 worker 处理改进**

当前没有可验证的 peer dispatch 通道。使用宿主 fresh GCM worker。不得调用待修 subagent 入口，不调用 `collab context` 注册，不 promote master。

parent 为 D2-A 创建独占 run、填实合同并确认笔记目录可写后，启动：

```sh
task_cwd=/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008
worker_run=/Volumes/Intel/playground/appsdk/.worker-runs/collab-context-identity-peer-crud-20261008/design-d2-admission-v3

env -u CODEX_SESSION_ID -u CODEX_THREAD_ID \
  -u CODEX_INTERNAL_ORIGINATOR_OVERRIDE -u CODEX_APP_TOOLS_PIPE_PATH \
  /opt/homebrew/bin/codex exec --profile gcm --ephemeral --json \
  --sandbox workspace-write -C "$task_cwd" --add-dir "$worker_run" \
  --output-last-message "$worker_run/result.md" \
  - < "$worker_run/worker-task.md" \
  > "$worker_run/events.jsonl" 2> "$worker_run/stderr.log"
```

这是 parent 后续命令，本 planner 不执行。`workspace-write` 供独占笔记使用；D2-A 产品目录仍按合同只读。若宿主能配置更窄权限，应将 W 设只读、仅 R 子目录可写。

具体改进：

- 每次只给当前有限节点；D2-A 不兼任完整 D2/D3。
- 开始即写输入/owner/当前步骤；首次交付的是映射表，不等最终回复。
- 每完成、失败或阻塞一个节点，即记录来源、版本、原错、下一步。
- parent 保存实际 PID、session/handle、启动命令、路由和退出码。
- 本次约定启动后约 3 分钟应有第一份阶段产物；没有时先查 notes/events/stderr 和实际进程，不能判失败。
- 连续约 5 分钟无新增证据时进入诊断；检查等待的工具、进程与最后节点。该时间是检查触发，不是产品 timeout 或自动重派依据。
- 对仍无可解释进展的有限节点，由 parent 受控停止该**精确自有 handle/PID**。记录中断原因，收取已有产物，再确认进程退出、无 writer、候选写范围。
- 确认旧 handle 已终止后才可重新派单。本次旧 `84142/84143` 已由最新 observation 和 run notes 确认停止，无需再杀或重复恢复。
- 缺 result 的任务保持 interrupted/INCOMPLETE；不能补写为 DONE。
- 旧 run 保留至 parent 收件和审核。无新增 heartbeat 文件，无 broad kill，无轮询超时即重派。

本轮不建议 parent 接手 D2/D3 设计正文。parent 只做接受、状态汇总、G23 注册和 review 编排。G23 是已确认的单一 registry owner，不改变 D2/D3 业务设计；如需改变合同必须回设计 owner。独立设计 reviewer 与 parent、planner、设计作者保持角色分离。

**10. 静态验收与独立设计 review 准入**

D2/D3 作者完成设计后，在 W 执行相应图校验：

```sh
dagpipe graph validate docs/dagpipe/collab-context.graph.json
dagpipe graph validate docs/dagpipe/collab-pane-route-reconcile.graph.json
dagpipe graph validate docs/dagpipe/collab-peer-lifecycle.graph.json
git diff --check
```

只执行本节点存在且修改过的图。未改 master/subscription 图复用有效结果；发生修改才补验。

G2/G3 完成注册后：

```sh
cargo test --manifest-path rust/Cargo.toml --locked --bins dagpipe -- --test-threads=1
cargo fmt --manifest-path rust/Cargo.toml -- --check
git diff --check
```

注册必须闭合 manifest path/ID、embedded、design IDs、operator inventory、相关测试。零项或 ignored 不算对应 gate PASS。图校验不等于 registry compile，更不等于权限和宿主行为。

每个设计包返回：

```text
状态：READY_FOR_DESIGN_REVIEW 或 BLOCKED
输入 HEAD 与相关 dirty 输入
合同/图的精确路径及 SHA-256
操作/状态/阶段/query/取消矩阵
每节点唯一 owner、源码映射、实现缺口
逐例 CLI/MCP 输入、预期输出及副作用
准确验证命令、退出码、非零测试数
registry 需求、原错、未知、资源与 notes 路径
```

parent 冻结这些输入后，分别创建：

```text
collab-d2-design-review-20261009-v3
collab-d3-ruc-design-review-20261009-v3
```

使用宿主 review MCP 的 milestone 入口：`profile:"oauth"`、`model:"gpt-6.1-sol"`、`mode:"base"`、绑定上述 HEAD。review context 必须列最新 observation、任务真源、本计划接收记录、精确合同/图/hash、registry 证据、O2/O5、D1 限定范围与未解决项。

设计 review 只审契约、DAG、owner、基础缺口、验收计划。只有有效 final、退出码及 controller 裁决才能接收 PASS。PASS 不代表基础已实现，也不代表 A6 或 installed/live 通过。

**11. A1–A12 与后续实施顺序**

| 验收 | 必须保留的行为终点 | 当前依赖 |
|---|---|---|
| A1 | 一次 context 自动登记/恢复、完整 snapshot、无额外探测、无 token 泄露 | D2→基础→F2→public/live |
| A2 | 精确 missing/source/template；补交前无身份写；一次真实补交后可复用 | D2/F2 |
| A3 | 未批准冲突明确拒绝并提供准确批准路径；不误选、不 mint | D2/F2 |
| A4 | stale peer credential/binding 一键批准恢复；指定身份与 scope；保留任务/mailbox；退役冲突 binding | D2/F2 |
| A5 | 原 master grant 恢复与显式替换分别验收；无隐式 promote；unknown incumbent 不锁死 | D2＋原 authority owner |
| A6 | 真 peer/managed 宿主、登记、route、工作；丢响应/重复无第二实例或假成功 | BLOCKED |
| A7 | 同源 Read；cwd 真实后续执行生效；身份/责任不变；非法/越权无副作用 | D3→基础→R/U |
| A8 | 责任 fence；精确宿主停止＋控制退役；失败明确；重复 receipt 一致 | D3→基础→Close |
| A9 | 全部支持写项真实派发、鉴权、完整持久回执；无空成功 | F1 局部候选；完整 managed 依赖 A6及 readback |
| A10 | context/help/MCP/源 Skill 同角色状态给同一准确可执行流程，完整 CRUD | 最终 D2/D3/Create 合同与行为 |
| A11 | replay 保留 identity/grant/task/mailbox；partial/丢响应可查；无重复 mint/wake/create | 各基础与组合，包括 A6 |
| A12 | project/app 隔离、grant owner、默认订阅、消费、选定 transport 保持 | 对应回归与最终 live |

后续按依赖推进：

1. DR2/DR3 对应 PASS 后，接收设计包中逐文件冻结的 **B23 基础实施合同**。先做 bootstrap admission、阶段持久化、replay/query、幂等/冲突及必要 fence。不得直接授予 v2 的宽泛源码 allowlist。
2. 基础公开契约单测通过后，实施 D2 identity 与 D3 R/U/C 核心。只有路径无重叠、接口依赖已满足时并行。
3. 单一 S23 owner 接 CLI/MCP/context。接收并保留当前 F1 dirty；不得重建控制事实或裁剪 `Resp`。
4. 获得允许真实 socket 的执行能力后，跑真实 public consumer。原 target 与断言保持；不重复同一 EPERM 探测。
5. A6 新证据解阻后，完成其独立规划、设计 review 与正式 Create，再建立 public managed fixture，补完整 F1 success/partial/repair/consumption/restart。
6. F4 修改源 `collab/skills/collab/` 与 operations/help/catalog。删除冲突和重复流程；全局副本只经官方安装刷新。Create 未完成时 A10 不能标完整通过。
7. 完成全部适用公开黑盒、官方安装和 controlled live，再进入独立实现 review。
8. 对精确最终候选完成正常 commit/PR/CI/受保护集成、remote 对应和自有资源收口。

未来 consumer 入口保留：

```sh
cargo test --manifest-path collab/Cargo.toml --locked \
  --test subagent_public_entry_cli -- --test-threads=1

cargo test --manifest-path collab/Cargo.toml --locked \
  --test identity_context_public_cli -- --test-threads=1

cargo test --manifest-path collab/Cargo.toml --locked \
  --test peer_lifecycle_public_cli -- --test-threads=1
```

后两者仍是待创建 target。D2/D3 设计包必须先冻结逐例输入、fixture、MCP JSON 和命令文件；target 不存在时不能执行或记 PASS。

正式安装由 delivery owner 使用 `scripts/install-global-collab.sh`。runtime 使用当时核实的 canonical installed binary。维护动作按项目正式通道与已有具体授权执行；Goal 授权不能替代正式目标 identity takeover 批准。

**12. 失败、交付资源与下一轮触发**

只停止受影响链：

- D2-A 无法安全到达原 receipt/query owner：返回具体缺失接口，停止 F2 上层实施。
- 设计必需宿主能力未知：可交草案及缺口，不能接收为完整设计准入。
- query 无 replay 后结果：停止依赖写入准入，不用日志/snapshot 推断控制事实。
- Update 只有 ACK：A7 不接收。
- Close 只有 archive ACK、`WorkerClosed` 或 record-only：A8 不接收。
- responsibility writer 未受 fence：Close 设计不接收。
- 新设计或 registry 验证失败：停止对应实施，不重做无关 D1。
- A6 无新能力证明：保持 BLOCKED，不 start/retry。
- 新 writer 或 dirty ownership 冲突：停止该路径，保留现场。
- 目标、scope、关键方案或依赖改变：补 observation，再 fresh plan。
- review/CI/hook/安装/Git 动作失败或结果未知：保留原错，从实际 receipt 确认，不盲重放。

parent 独占候选、汇总、registry、安装维护、Git 集成和资源生命周期。各 worker 只负责自己的子进程与 run。必要证据收件、审核、归档且进程已停止后才回收临时记录。当前 dirty worktree、主树 dirty、共享 daemon/AppServer、其他任务资源不能清理。

下一轮触发：

- D2-A 返回：接收映射，按已接受计划推进 D2-B，不为普通节点推进重 plan。
- D2/D3 设计通过：接收精确 B23/实施合同；若引入了本计划未覆盖的关键方案，补观察再规划。
- socket 执行能力恢复：补原 public consumer；不自动解除 A6。
- A6 出现符合阈值的新证明：单独 fresh Create plan。
- 本轮设计交付完成而总目标仍未完成：parent 将实际结果、裁决和剩余依赖交下一轮 planner 更新。
- 只有四项能力、A1–A12 及交付终点全部闭合后，才关闭 Goal。

**13. 经验修订候选**

| 先验/旧条目 | 新证据与替代解释 | 后验及修订建议 |
|---|---|---|
| 旧 D2/D3 全包派单可持续推进 | 两个独立 session 都无结果；run notes 记录长期无新事件。不能证明 provider/model 根因 | 本任务派单改成先交有限产物、精确 handle 核验与受控终止；效果待下一执行验证 |
| anchor 正文“只有 master 归属需裁决” | A4/A5 要求 peer/master 批准恢复；当前正文直接冲突 | 修项目唯一 anchor 正文，保留自动恢复并引用 D2 |
| minimal interaction 正文禁止 approval | 当前权威目标明确增加批准恢复 | D2 同步修其准确 schema/admission 段落，避免两份合同互相否定 |
| pane fresh registration 无条件跨 scope 接管 | 当前图直接这样描述；与 scope/ownership/批准合同冲突 | 修 writer 裁决描述；reconciler 继续非驱逐 |
| `WorkerClosed` 即完整 Close | reducer 与宿主路径证明不足；重复源码读取不算独立 live 证明 | D3 明确双终点、旧 receipt 兼容与 fence |
| F1 局部绿测可完成交付 | checkpoint 明确缺 daemon-backed、managed、installed/live、实现 review | 保持局部候选；不缩小总验收 |
| 空列表能排除未知创建 | 原 Native receipts 与 schema 已反证 | 保留 A6 BLOCKED；无新证据不重复观察 |

这些候选归本任务合同、计划和源 Collab 文档。现有全局规则已覆盖独立角色、证据复用、精确资源回收和 unknown 不重放。本轮不新增全局规则或长期 memory。经验候选交独立 reviewer 复核，parent 按授权更新唯一正文。**本轮没有更新任何规则或记忆。**

