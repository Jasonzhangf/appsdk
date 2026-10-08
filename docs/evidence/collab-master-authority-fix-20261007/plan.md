**状态：READY**

这是可派单计划。READY 不代表设计准入 PASS、功能完成或架构 review PASS。产品代码写入仍须等待修订后的项目 DAG 校验及独立设计 reviewer PASS。本 planner 只提供计划，不实现、不 review、不执行测试、不安装或重启、不集成、不管理资源。实际 profile/model 由 parent 的启动证据确认，不能以本正文自报身份替代。

**输入绑定**

| 项目 | 绑定 |
|---|---|
| 任务、轮次 | `collab-master-authority-fix-20261007`，首次独立规划 |
| Observation | `/Volumes/Intel/playground/appsdk/.worker-runs/collab-master-authority-fix-20261007/observation.md`，已完整读取 |
| 源码 base | `7350fbf6b020b1464d531337c7b2f6b8fa5de6f2` |
| 工作树 | `/Volumes/Intel/playground/appsdk/collab-master-authority-fix-20261007` |
| 分支 | `codex/collab-master-authority-fix-20261007` |
| 已安装 CLI 观察 | `collab 0.2.0253`；不据此认定 daemon 已加载相同源码或 binary |
| 有效历史 | 本任务 `audit.md`、`audit-evidence/`、`parent-notes.md`；既有 baseline 测试及 DAG 拓扑校验证据 |
| 缺陷 | `1f79966`，保持 OPEN，直至 main 与正式 runtime 验收完成 |
| 前一计划 | 无；审计不是独立计划或设计准入 PASS |
| 本次变化 | 增加独立 oauth planner；目标、授权和源码 base 未变 |
| 执行分工 | GCM implementation 写产品；独立设计 reviewer 审编码前 DAG；最终 reviewer 与 planner、实现者分离；parent 持有正式 runtime、集成和收口责任 |

以下命令默认 cwd 为上述工作树。证据根记为：

```text
RUN=/Volumes/Intel/playground/appsdk/.worker-runs/collab-master-authority-fix-20261007
```

`RUN` 是本任务已有记录位置。执行者在自己的记录子目录写日志与节点笔记。parent 汇总到已有 `parent-notes.md`，不把聊天作为阶段状态真源。

**目标与判断**

首轮交付完整可用主线：已认证的 tmux master 在 agent 运行事实为 `Unknown` 时可执行控制权限；明确批准可替换旧 master 或清除当前 route 的授权；同主体端点恢复保留原批准，新主体不继承；status、context、board/panel 显示同一授权持有人；正式安装、重启后结果持久有效。

本轮保留 **canonical project + app scope** 的现有授权隔离合同。所有授权回执和公共投影明确显示这两个 scope。同项目其他 app scope 的不同持有人或空授权不是错误，不能把其空状态解释为整个项目没有 master。本轮不改成全项目唯一 master，不迁移全局 schema。

已确认事实：

- `worker_identity_presence` 的 tmux 分支把 tmux 送入不支持它的 AppServer oracle。生产 oracle 对 tmux 返回 `TMUX_ENDPOINT_NOT_QUERYABLE`。
- `live_master_id`、`live_master_worker_snapshot` 把该观测用于授权判定。board、任务控制、订阅和部分跨项目控制沿用了这条边。
- `promote --approval` 已支持不探测旧 master 的明确替换。应复用该路径。
- `project_route_actor` 已验证 token、scope、当前 binding 和 endpoint generation。它是本轮拒绝旧请求的基础。
- `typed_dispatch` 已在同主体 generation 替换事务中撤销旧 generation grant，并重新签发当前 generation grant。没有必要增加稳定主体数据库或第二套授权计数器。
- status/context 的 live 分流及 legacy fallback 形成了不同授权读模型。
- 普通 AppSDK 初始化及 fresh governance 初始化没有清除 master 的合同。不能用初始化暗删授权或业务状态。

可证伪的根因判断：**通信观测被用作授权成立和控制操作的前提，导致 tmux 的不可查询状态阻断已认证 master。** 基线公开 CLI 红测与候选同入口绿测必须证明此判断。若基线用例不能触发该门，先修正 fixture 或补最小观察，不把源码推断当完整因果证据。

尚未确认的关键输入及最小观察：

| 未知 | 最小观察及 owner | 对计划的影响 |
|---|---|---|
| 正式 daemon 的当前 PID/socket/descriptor 是否一致 | parent 复用 runtime worker 结果；到维护边界刷新精确 socket owner、loaded binary、descriptor 和官方状态 | 不阻断隔离实现；不一致时阻断正式 down/up 及后续交付 |
| 用户当时 panel 的真实响应 | 不追补历史截图；在本轮 fixture 与 installed replay 读取真实 panel GET | 不得宣称历史 panel 显示在线 |
| 本目标实际旧授权是否允许执行 clear | parent 绑定用户批准的 canonical project、app scope 和当前持有人 | 测试可在隔离项目执行；正式 clear 仅在明确批准范围执行 |
| legacy 授权字段的 replay 接线 | 实现者只追 `GlobalMasterRevoked`、历史 `MasterAssigned/RootAssigned` 到 reducer 的受影响路径 | 必须保证 clear 后重启不复活；不扩大成全量历史迁移 |

唯一授权 owner 是现有 daemon typed grant/reducer 事务。CLI 是请求适配层。context/status/board 是公共投影。adapter 与 recipient 是通信事实和发送能力 owner。AppSDK init 不成为新的授权 owner。

**DAG：沿现有图作最小修订**

复用三个项目图：

- `docs/dagpipe/collab-master-authority.graph.json`
- `docs/dagpipe/collab-context.graph.json`
- `docs/dagpipe/collab-dashboard.graph.json`

授权图保留三节点形状。将第二节点语义从“转移”扩展为“按本次意图原子变更授权”，节点内部处理 promote、delegate、clear，不增加 pending workflow：

```text
解析明确作用域及认证主体
  → 原子替换或清除该作用域授权
  → 返回持久回执或明确错误
```

图与对应设计正文须写明：

- promote：当前已认证 peer + 非空 approval，原子替换当前 route 的 incumbent。
- clear：当前已认证 peer + 非空 approval，原子撤销当前 route 的 grant；不要求调用者先成为 master，不探测 incumbent。
- delegate：当前已认证 master 可变更持有人；授权转移与后续通知分别给出结果。目标必须是该 route 已注册且具有有效当前 binding 的主体。通信未知不能成为授权转移的 live 门；实际通知仍执行 recipient 安全检查。
- 认证或批准失败：返回明确拒绝，授权和业务状态不变。
- 持久化失败：沿现有错误链返回失败；不能先投影成功。
- 提交前取消：无授权变更。提交后响应丢失：通过当前 route status 查证实际持有人；不新增操作查询框架，不盲目重放替换。
- clear 重复调用：返回成功及“已为空”的回执，不重建授权。
- 本次请求不创建独立长期资源。fixture 清理由创建者完成。

context 图仅修订身份恢复与授权投影合同：

```text
已验证同主体恢复 → 更新当前端点与请求代际 → 保留原批准的当前 grant
新主体注册       → 建立自己的 binding    → 不继承其他主体 grant
```

删除 description 中“unknown liveness 一律阻断身份恢复”的笼统授权约束。保留身份锚点、歧义、跨项目、endpoint 验证及真实冲突的拒绝合同。

dashboard 图保留现有四节点。授权节点只读当前 route grant；投影节点将授权、地址事实和 agent 运行事实分开。HTTP GET 继续只读。

实现者执行以下图校验并保存每次退出码及输出：

```sh
dagpipe graph validate docs/dagpipe/collab-master-authority.graph.json
dagpipe graph validate docs/dagpipe/collab-context.graph.json
dagpipe graph validate docs/dagpipe/collab-dashboard.graph.json
```

在现有设计文档补中文业务语义图、节点 owner、公开入口和成功/失败终点。图中未实现边标为待实现。三个 validate PASS 只证明图形与绑定形状；独立设计 reviewer 须对本计划对应的最终图修订取得 PASS，之后才能写产品代码。

**增量**

本轮一个完整增量，按节点逐步完成：

1. 闭合设计与公开 CLI 红测。
2. 修授权判定、明确清除、同主体恢复及统一投影。
3. 完成受影响 consumer 黑盒、官方安装与正式 runtime 验证。
4. 独立架构 review、main 集成、main 重建重启及收口。

安全、旧请求拒绝、clear 的业务数据保留、重启持久性和 consumer 验收全部属于首轮，不后移。

非目标：host reset、权限数据库清理、journal/mailbox/tasks 销毁、全局 schema 迁移、业务消息或任务状态机重写、新 heartbeat/授权服务、新管理 panel 写接口、非目标 daemon 生命周期、原主树 dirty `docs/collab.md`。

后续只在新事实要求更换 scope 合同、历史格式无法安全解释、或现有同主体认证不足时开启新轮。不得借这些可能性扩大本轮。

**具体方案、保留与消融**

1. **把授权判断收敛到 current grant。**
   使用现有 `server_route_scope`、`current_master_grant` 和 `project_route_actor`。控制操作在 mutation lock 内核对当前持有人；不能只依赖锁外 snapshot。若需要共享辅助函数，替换现有权限辅助函数的语义与名称，不叠加第二套权限链。

2. **promote 保留明确替换语义。**
   修 CLI help、回执和 Skill 中“只有无 live master 才能 promote”的旧指引。批准文本不是身份凭据；身份仍由现有认证入口验证。不要增加 `force` 旁路。

3. **新增 `collab master clear --approval "<text>"`。**
   在 `main_cli.rs`、`main.rs`、`proto.rs`、daemon dispatch 和 mutation admission/actor 提取处完整接线。clear 必须进入 `project_route_actor`，不能因它清除的是其他 holder 而绕过 token/current binding 校验。
   复用现有 route 内 revoke event 生成逻辑及持久提交，和 promote 共用 scoped revoke owner。clear 只撤销匹配 project/app scope 的授权。回执包含 scope、前持有人、当前空持有人、是否本来为空及现有提交版本信息。不得新增第二套授权 epoch。
   历史兼容字段不能让旧授权在 clear 后成为 fallback。必要的历史 event adapter 留在 replay 边界；公共授权查询只读当前 typed grant。

4. **同主体恢复复用已有事务。**
   删除 `part_06.rs` 的 master live/unknown 恢复围栏及仅为绕过该围栏存在的 same-pane/same-DSH 授权例外。保留 token 校验、已验证身份锚点及 `same_principal` 合同。
   保留 `part_02.rs` 已有同主体 reissued grant 事务及 host-route commit/rollback。grant 更新到新 generation，原批准主体与批准来源不变。新主体、跨 scope 或仅占用同 pane 不能获得前主体授权。不要把 `bind_runtime` 改成对所有重绑都无条件保留 grant。

5. **统一公共授权投影。**
   status、context、board/panel 使用同一 grant 投影，至少包含 `worker_id`、canonical project、app scope 和现有批准来源。通信观测作为独立字段输出。
   已有 holder 在 `Unknown/Cold/Missing` 时仍显示为 holder。授权查询成功不能因通信 probe 失败返回“无授权”或“授权未知”。route 解析或 reducer 读取失败仍返回明确错误，不能用 `.ok().flatten()` 伪造空授权。
   删除 `recorded_unusable` 作为另一授权模型的用途，以及 `state.master_*` 的公共授权兜底。若保持字段兼容，只能由同一当前 grant 投影生成，不能再参与决策。
   tmux 可以报告 pane 地址可用，但 agent 运行状态仍为 unknown。移除 tmux → AppServer oracle 的无效边，不把 pane 存在改成 `IdentityPresence::Present`。

6. **受影响 live 授权门必须删除或改为 current grant。**

| 位置 | 处理 |
|---|---|
| `part_07.rs` 的 `verify_master_actor` | 当前认证主体与当前 grant 比较；删除 caller live 条件 |
| `board_handlers.rs` 的 Publish/Invite/Withdraw 前置 gate | 删除 `BOARD_LIVE_MASTER_REQUIRED`；保留锁内 actor、master、revision/owner 检查 |
| `part_06.rs` master 恢复围栏 | 删除授权 live/unknown 与传输专属绕门例外 |
| `part_06.rs` deadline/master-idle 订阅资格 | current grant 决定权限；原订阅地址及生命周期约束保留 |
| `part_09.rs` review accept、integrated、close、finalize cleanup | current grant 决定 master 责任；保留任务 owner、候选 main 证明、pending merge、清理证据和孤儿 owner 判断 |
| `part_08.rs` 的 `task_integration_authorized` | 删除 master live 条件；保留 task owner 路径 |
| `subagent.rs` parent-or-master 控制 | master 分支读取 current grant；child readiness 的绑定约束保留 |
| `main.rs` cross-project send 的 sender `endpoint_live` gate；`part_04.rs` source master gate | 发送方权限由当前 source grant 决定；保留 source reducer 对批准来源和 scope 的核对 |
| status/context、role brief、operations 指引 | 删除通信 unknown 触发“重新赋权”的建议；空 grant 才投影未指定 master |
| timers、keepalive、presence edges 中的 master 选择 | 授权对象选择取 current grant；实际唤醒仍走独立通信检查，不把 live helper 改名后全局替换 |

逐个核实 `live_master_id` 与 `live_master_worker_snapshot` 的调用用途。权限用途收敛到 grant；纯通信用途保持独立且命名准确。完成后删除已无真实消费者的 live 授权 helper 和旧测试断言。

7. **以下检查必须保留。**

- `project_route_actor` 的 token、project/app scope、唯一当前 binding、session/thread/runtime 和 generation 拒绝。
- 目标主体注册、route 归属、地址唯一性与最新 binding 校验。
- tmux socket/server/session/pane 元组、pane ownership、发送前地址核对，以及禁止向 shell/absent/unknown agent 注入输入的现有规则。
- AppServer 当前线程校验、原生启动/steer/queue 规则、Cold 与 Missing 区分。
- DSH gateway 的 runtime/agent/session/cwd 核对、实际 enqueue 合同及明确失败。
- delegation 后通知、跨项目接收方、live closure 的 recipient 通信检查。
- durable mailbox、recv/consumption receipt、订阅 owner 和 wake attempt 规则。
- force close/finalize 中“owner 明确 Missing”与 Unknown 的区分。master 通信未知不能被解释为无 master，并开放 peer 的孤儿处理权限。

错误隔离：身份、scope、批准或旧 binding 错误只拒绝本请求；不改状态。通信查询失败只影响观测或发送，不能撤销授权。存储提交失败终止本次 mutation 并暴露原错。共享状态损坏或授权歧义必须明确失败，不随机选 holder。

**执行任务**

所有任务完成、失败或阻塞时，owner 立即在已有独占笔记写：

```text
时间/节点｜结论或状态｜证据路径｜输入 SHA 与必要环境｜下一步
```

| ID | 依赖、owner | 允许 / 禁止路径 | 步骤、命令和完成条件 |
|---|---|---|---|
| P00 设计闭合 | 本计划；GCM implementation 修图，独立设计 reviewer 审 | 允许相关 `docs/design`、三个现有 graph；禁止产品源码及其他 owner 文件 | 按上述最小修订落盘；执行三个 `dagpipe graph validate`；parent 把最终图和计划交设计 reviewer。完成 iff 校验 PASS 且对应修订的独立设计 PASS 已记录。证据回传 `RUN/implementation/` 与设计 reviewer 既有目录。 |
| P01 公开入口红测 | P00 PASS；GCM implementation | 允许 `collab/tests`、必要 consumer 测试；禁止产品逻辑、正式状态 | 在现有 tmux/AppServer fixture 增加 `master_authority_` 用例；补最小 DSH 公开 consumer 用例。使用真实隔离 daemon 和真实 CLI/公开请求，不直接调用 handler 代替。在 base 产品代码上执行下列命令，保存失败断言。完成 iff 已有行为缺陷有实际行为红测，clear 新命令缺失单独记录。 |
| P02 最小实现 | P01 有效红测；GCM implementation | 允许 `collab/src`、`collab/tests`、相关设计/DAG、`collab/skills/collab`；禁止原主树 `docs/collab.md`、全局配置、正式 runtime、其他任务状态 | 逐文件核实后 apply_patch。按方案接线 clear、权限、恢复、投影和消融。完成 iff P01 同入口绿、认证拒绝和数据保留断言通过、无双授权读模型。 |
| P03 作者验证 | P02；GCM implementation | 同 P02；仅隔离 fixture | 执行格式、库与 consumer 命令；真实 GET 验 panel；保存按矩阵分项结果。完成 iff 受影响成功/失败/副作用全部通过。把所用 fixture、binary 和 SHA 回传 parent。 |
| P04 最新 main 候选 | P03；parent 持有候选集成边界，implementation 修发现的问题 | 当前候选工作树；禁止覆盖 dirty 主树 | fetch 最新 main，将修复组合到它；重跑因更新而失效的 P03。形成可追溯候选 commit，记录精确 SHA。若无变化，复用有效行为证据。完成 iff 候选来源和验证绑定明确。 |
| P05 候选正式 runtime 验证 | P04；parent | 官方安装、批准的 Collab down/up；禁止 reset、迁移、非目标服务、生产测试消息 | 官方 install → installed digest → down → up → context → MCP initialize → installed 黑盒。先刷新 runtime owner。完成 iff 新 PID/socket/loaded binary、持久数据和真实入口证据齐全。 |
| P06 独立架构 review | P03–P05 全 PASS；parent 派独立最终 reviewer | reviewer 只读候选与证据 | 审精确候选 SHA；检查 owner、消融、scope、认证、恢复、clear 和 consumer 回归。完成 iff PASS。发现行为缺陷退回作者；改变方案须 observation/replan。 |
| P07 main 与正式重建重启 | P06；parent | clean main、官方 runtime；禁止 hook bypass、强推、覆盖 dirty 文件 | 边界 fetch；确认无更新、无 pending 集成锁；合并、等价核对、push、远端核对；从 main 官方 install，再 down/up/context/MCP initialize/installed replay。完成 iff main 与已验候选等价，远端回执及 main runtime 验收通过。 |
| P08 证据与自有资源收口 | P07；各创建者，parent 汇总 | 只回收本任务自有且无用途资源 | 先归档必要证据到既定 `docs/evidence/collab-master-authority-fix-20261007`，再按 owner 回收 fixture、临时进程、任务 worktree 和记录目录。完成 iff 清理核对成功；之后关闭 bug。 |

P01/P03 的完整执行命令：

```sh
cargo test --manifest-path collab/Cargo.toml --test tmux_recv_e2e master_authority_ -- --nocapture
cargo test --manifest-path collab/Cargo.toml --test appserver_two_tui_integration master_authority_ -- --nocapture
cargo test --manifest-path collab/Cargo.toml --test dsh_master_authority_cli -- --nocapture
```

`dsh_master_authority_cli.rs` 是本轮最小公开 consumer 测试文件。复用已有 DSH gateway 协议 fixture 的消息合同，但通过隔离 daemon 的公开请求及 CLI 验证。不能只复制现有直接 handler 单测，并把它命名为 E2E。

P03 补充命令：

```sh
cargo fmt --manifest-path collab/Cargo.toml -- --check
cargo test --manifest-path collab/Cargo.toml --lib
cargo test --manifest-path collab/Cargo.toml --test master_status_cli
cargo test --manifest-path collab/Cargo.toml --test tmux_recv_e2e -- --nocapture
cargo test --manifest-path collab/Cargo.toml --test appserver_two_tui_integration -- --nocapture
cargo test --manifest-path collab/Cargo.toml --test dsh_master_authority_cli -- --nocapture
```

首次完整受影响 suite 通过后不重复全跑。后续只按代码、main、配置或 runtime 变化重跑失效项。

P04 命令：

```sh
git fetch origin main
git rev-parse origin/main
git status --short
git log --oneline origin/main..HEAD
```

有新 main 且候选尚未提交时，先按项目 hook 提交实现，再在本独占候选树组合：

```sh
git merge --no-edit origin/main
```

冲突停止该节点，保留原错交 parent；不强行选择一边。candidate commit 的提交和 hook 按项目现行 gate 执行，不规划 `--no-verify`。

**黑盒验收矩阵**

测试名前缀统一为 `master_authority_`，使 P01/P03 可定向执行。所有用例绑定输入 SHA、fixture project/app scope、binary、输出及证据路径。

| 用例 | 公开输入与前置条件 | 外部断言 |
|---|---|---|
| B01 tmux Unknown 控制 | A 经真实 context 注册并 promote；tmux agent 运行事实 unknown；A 执行 board publish/withdraw 及受影响 master 控制 | 控制成功；status/context/board holder 均为 A；agent 不显示 online。base 在 live gate 处红，候选绿。 |
| B02 明确替换 | A 为旧 holder；B 已认证；B 执行 `master promote --approval "fixture replacement"` | B 立即取得控制权限；A 控制拒绝；A 的 peer、task 和 mailbox 保留。旧 pane 存在或不可查询不阻断替换。 |
| B03 clear 与幂等 | 有 holder、其他 peer、公开 task 和未消费消息；认证 peer 执行 `master clear --approval "fixture clear"`；再 clear | 两次成功，第二次报告已为空；所有投影为空；task/peer/原消息及消费状态保持；旧 holder 控制拒绝。 |
| B04 批准失败 | promote/clear 无参数、空白 approval | 非零或公开错误；holder、task、message、peer 不变。缺参数和 daemon 空白拒绝分别覆盖。 |
| B05 认证与 scope | 错 token、跨项目、跨 app scope、旧 runtime/binding/generation 请求，走公开 socket 协议 | mutation 明确拒绝；两个 route 的授权及业务状态不变；不打印 token。 |
| B06 同主体恢复 | A 为 holder；由身份 owner 验证 A 新端点，更新 generation；保留旧请求 | A 当前端点保持原批准并可控制；旧请求拒绝；新端点的 grant 与 binding 一致。 |
| B07 新主体不继承 | 同 pane 换为新主体或其他已注册主体占用地址 | 新主体仍为 peer；不能仅因 pane/名字相同成为 master；旧端点请求拒绝。 |
| B08 重启持久性 | 替换后 down/up；clear 后再次 down/up；从公开 context/status/board 读取 | 新 holder 保留；clear 不复活 typed 或 legacy holder；业务数据保持。 |
| B09 scope 展示 | 同项目两个 app scope，以及另一个项目 | 每个入口明确返回自己的 project/app scope；替换或 clear 只改变指定 route。 |
| B10 投影及 panel | 同一次稳定授权状态下读取 status、context、board show、真实 dashboard GET | holder 一致；panel Master 角色与 agent unknown 分开；GET 没有新增写权限。 |
| B11 AppServer consumer | 已有 two-TUI fixture：运行、Cold、query error；执行授权控制及实际原生消息路径 | holder 不随通信状态消失；控制权限按 grant；AppServer 启动/steer/queue、recv 和 durable receipt 仍符合原合同。 |
| B12 DSH consumer | gateway running/inactive/unreachable；同主体换 gateway 地址；旧 runtime 请求 | 授权保持，观测如实输出；当前请求可控制；旧 binding 拒绝；实际发送按 gateway enqueue 合同明确成功或失败。 |
| B13 接收方安全 | shell/absent/unknown recipient；旧 tmux 地址；AppServer-bound peer | 不向未知归属 shell 注入；不把 AppServer 失败改走 tmux；消息与 wake/consume 结果不伪造。 |
| B14 相邻任务权限 | holder Unknown 时 review accept/控制；替换后旧 holder 试控制；非 holder 操作 | master 责任按当前 grant；pending merge、owner、candidate/main 证明和清理证据门保留；无权限主体拒绝。 |
| B15 并发变更 | 同 route 两个已批准 mutation 经公开入口并发发送；同时读取状态 | 按现有事务顺序仅一个当前 holder；读到完整前态或后态；不出现双 holder 或部分 clear。 |

B05 可在 fixture 内读取自己的 credential 构造公开请求。不得读取或复制既有用户 credential。数据保留断言比较公开任务内容、peer 身份和原消息 ID/正文/状态，不能只比较计数。

P01 新增的 clear 在 base 上“命令不存在”仅证明新增能力缺失；B01 的行为失败才是本缺陷的根因红测。既有 baseline `master_status_cli` 的两项成功不重复作为本缺陷红测。

**正式 installed 验证命令及证据**

P05/P07 均由 parent 执行。候选 fixture 增加一个测试 binary 选择入口，例如 `COLLAB_TEST_BINARY`，只用于从同一 consumer 用例选择 debug 或 canonical installed binary；不新增测试框架。fixture 继续使用独立 `COLLAB_STATE_DIR`、隔离 project、自己的 tmux/gateway，不触碰真实用户消息。

从当前安装执行环境派生路径：

```sh
printf '%s\n' "$HOME"
printf '%s\n' "${CARGO_HOME:-$HOME/.cargo}"
command -v collab
command -v collab-mcp
```

parent 记录结果，再定义：

```sh
collab_bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
```

候选官方安装：

```sh
scripts/install-global-collab.sh
"$collab_bin_dir/collab" --version
shasum -a 256 collab/target/release/collab "$collab_bin_dir/collab"
shasum -a 256 collab/target/release/collab-mcp "$collab_bin_dir/collab-mcp"
```

安装器已经执行候选/installed binary 等价及 Skill byte-for-byte 校验。保存该证据，不再建立第二套摘要门。

正式 runtime 的维护 cwd 使用 parent 已核实的 service scope root；不从 daemon cwd 猜 scope。记录维护前精确 PID/socket/descriptor，再执行一次官方窗口：

```sh
"$collab_bin_dir/collab" down
"$collab_bin_dir/collab" up
"$collab_bin_dir/collab" context
"$collab_bin_dir/collab" status --all
```

context 由 owning 会话执行；缺事实只补返回的 `required_fields`。不得为验收注册假用户身份。MCP initialize：

```sh
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}' |
  "$collab_bin_dir/collab-mcp"
```

断言 initialize 返回 `serverInfo.name=collab`、当前安装版本和对应 protocolVersion。

installed consumer 回放的 cwd 为候选工作树：

```sh
COLLAB_TEST_BINARY="$collab_bin_dir/collab" cargo test --manifest-path collab/Cargo.toml --test tmux_recv_e2e master_authority_ -- --nocapture
COLLAB_TEST_BINARY="$collab_bin_dir/collab" cargo test --manifest-path collab/Cargo.toml --test appserver_two_tui_integration master_authority_ -- --nocapture
COLLAB_TEST_BINARY="$collab_bin_dir/collab" cargo test --manifest-path collab/Cargo.toml --test dsh_master_authority_cli -- --nocapture
```

这些用例证明 installed bytes 的公开行为。正式服务还必须证明新 PID/socket、loaded canonical binary、实际 context 与持久状态保持；隔离 fixture 不能替代正式服务证据。正式项目仅执行已批准的目标操作和只读核对，不注入测试消息。若 parent 尚无本目标 clear 的明确批准范围，不执行正式 clear；隔离 clear 验收继续完成。

**并行安排与真实依赖**

- 首个 ready 层：implementation 仅修订设计产物；runtime worker 完成只读正式能力预检；设计 reviewer 可审已提交的最终修订。写入范围不重叠。
- 设计准入后：implementation 按 P01→P02→P03 连续执行。parent 同时准备 runtime 证据绑定、review 合同和 clean main 集成条件。
- P03 内独立 suite 可并发运行，但每个 fixture 必须独占 project、state root、tmux socket 和 gateway socket。
- 不拆第二个产品写 worker。授权、恢复、投影共用同一 owner 及源码范围，重复派写会造成所有权冲突。
- 官方 installer 使用全局 build-version 锁；正式 runtime 维护由 parent 独占。P05 必须等精确候选及作者验证通过。
- P06 必须等 P05；P07 必须等 P06 PASS；P08 必须等交付证据已归档。不能用并行缩短这些真实依赖。

**失败、停止和 replan**

- 设计 reviewer FAIL：停在图修订。不得写产品代码。
- base B01 不红：停在最小复现校正。只观察该入口的 binding、grant 和实际拒绝点，不重新全仓侦察。
- 同主体必须靠 pane 名称或未验证锚点才能恢复：禁止实现隐式继承；补身份连续性的最小观察并 replan。
- clear 需要删除 journal、mailbox、task 或全局 reset 才能生效：方案不合格，回唯一 reducer owner 修复。
- 新事实要求改 project/app scope 唯一键、schema 或任务生命周期：超出本轮，补 observation 并重新规划。
- 正式 down 返回错误或 `DAEMON_UNKNOWN`：先核对实际 PID/socket/descriptor 变化。禁止盲起第二 daemon，禁止任意进程清理。只阻断 P05/P07 的 runtime 链。
- 测试失败但方案不变：作者定位并修复，重跑受影响项；不重复 planner。
- review 发现行为缺陷：退回作者 debug/E2E；新 SHA 重新审。不能让 reviewer 代作者补测试。
- main 更新使候选变化：重新组合，重跑失效验证及 installed evidence，再 review 新候选。
- merge 冲突、push 拒绝、CI/hook 失败：停止受影响集成，保留精确错误交 parent；禁止强推或 bypass。
- main 安装后确认本改动造成回归：bug 保持 OPEN，parent 按已授权恢复 DAG 用可追溯 `git revert`，从回退 main 重建并验证恢复，再在同一 bug 下重新观察和规划。不得 reset 历史。

**review、main 交付与资源终点**

架构 review 准入包必须包含：精确候选 SHA、最终 graph/design PASS、base 红测、作者开发测试与 consumer E2E、官方安装版本及等价证据、正式 down/up/context/MCP initialize、installed 黑盒及数据保留结果。缺一适用项记 `INCOMPLETE`，不启动架构 review。

parent 从 `git worktree list --porcelain` 识别真实 main 工作树，不猜路径。原主树已有 dirty `docs/collab.md`，不得在该 dirty 树强行集成。已有 clean main 集成路径可复用；若不存在，parent 按允许的外置 worktree 边界建立专用集成树，禁止清掉用户 dirty 文件。

在核实的 clean main cwd：

```sh
git fetch origin main
git status --porcelain
git rev-parse HEAD
git rev-parse origin/main
```

确认边界条件后合并可追溯候选。若项目没有要求 merge commit，优先 fast-forward：

```sh
git merge --ff-only "$candidate_sha"
git diff --exit-code "$candidate_sha" HEAD
git push origin main
git rev-parse HEAD
git ls-remote origin refs/heads/main
git status --porcelain
```

`candidate_sha` 必须来自已验且 review PASS 的候选记录。main 内容等价、远端 SHA 回执和 clean 状态保存为集成证据。随后从该 main cwd 执行 P05 的官方安装、维护和 installed replay，形成 main runtime 证据。此 Collab 范围没有 Android 客户端、emulator 或 OTA 产物；这些节点明确不适用，不创建伪造 bundle。

资源由创建者回收。parent 只汇总并验收自有资源结果。必要证据先归档，禁止删除共享证据、他人进程、既有 fixture 或 dirty 文件。worktree 清理只移除本任务已无用途的工作树：

```sh
git -C "$verified_repo" worktree remove /Volumes/Intel/playground/appsdk/collab-master-authority-fix-20261007
git -C "$verified_repo" worktree list --porcelain
test ! -e /Volumes/Intel/playground/appsdk/collab-master-authority-fix-20261007
```

`verified_repo` 从当时工具结果取得。dirty 导致 remove 失败时保留该树，记录 owner、路径和原因，标未收口，禁止强删。独立集成树及测试资源按各自创建记录核销。临时计划与 worker 记录由 parent 归档后按 owner 清理，planner 不执行此节点。

**下一轮与总目标停止条件**

仅在 scope、身份连续性、存储合同、关键方案或依赖发生实质变化时开启下一轮。新 observation 只补变化事实、失败入口和受影响证据，不重复已有有效审计。

当首轮矩阵通过、独立架构 review PASS、main 与远端回执成立、main 官方重建重启及实际入口验收通过、批准范围内旧授权处理完成、自有资源已收口时，关闭 `1f79966` 并停止。任何适用证据或清理终点缺失，报告 `INCOMPLETE`；不得以源码测试或设计结论替代交付完成。
