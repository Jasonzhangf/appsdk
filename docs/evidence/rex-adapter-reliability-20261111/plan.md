# Collab Rex 适配可靠性修复执行计划

## 目标与交付边界

修复 Collab Rex 通道的四项实际行为：

1. 通知正文和提交用的 CR 在一次 `terminal.write` 中排队，消除两次调用之间丢失 Enter 的窗口。
2. `worker snapshot` 通过官方 `capture` 读取 Rex terminal，并严格限制公开返回行数。
3. Rex 通知拒绝使用 `REX_NOTIFICATION_REJECTED:` 标签，同时保留底层 Rex 原错。
4. Rex 状态按当前 foreground owner 和有效记录投影。`idle`、`done`、`error`、未知 state 和过期 owner 均不能产生 `working`。

完整交付顺序为：设计修订、红测、唯一 owner 实现、聚焦回归、真实入口验收、一次独立 milestone review、提交、PR、合并到 `origin/main`、远端核对和自有资源清理。

本计划不授权修改生产身份、daemon 或订阅。不运行 `collab context`、`collab recv`、ACK、mailbox cleanup/deliver/close，不执行广谱 Collab 测试。全局安装、生产 daemon 生命周期及权限变更不属于本次执行范围。

## 依据、事实与待补证据

当前有效需求已经从 `.appsdk/requirements.json` 的变更记录中读取：

| 需求 | 当前版本 | 本次约束 |
| --- | --- | --- |
| `req-minimal-governance` | 1 | 每项机制和测试必须对应实际失败模式；不增加第二套治理设施 |
| `req-collab-communication-notifications` | 6 | 入队、adaptor 接受、目标接收、回复和消费分别取证；后台状态由 daemon 维护 |
| `req-collab-identity` | 2 | 身份由现有唯一 owner 管理；观察和通信错误不能改写身份 |
| `req-command-logs` | 1 | 保留可审计的命令、调用和结果证据 |

当前笔记为 `docs/evidence/rex-adapter-reliability-20261111/note.md`。用户已提供 API revision 11 和隔离 Enter 探针结果。Planner 没有重新执行该探针。

源码已确认：

- `notify` 当前执行 `write_text_until`，再执行 `send_enter_until`。
- 第二阶段还会再次核实 service，增加两阶段之间的失败窗口。
- `handle_worker_snapshot` 已校验 `1..=200`，但将 Rex 放入 unsupported 分支。
- `notification_rejected_label` 没有 Rex 分支。
- `RexProgramStatusRecord` 没有保存 owner。
- monitor 当前只接受单条记录；`working` 映射为 Busy，其他多数状态映射为 Unknown。
- 公开 view 已将 Busy、Idle、WaitingUser、Unknown 分别映射为 `working`、`idle`、`waiting_user`、`unknown`。应先修 monitor，不预先改写公共状态层。
- 官方 `rex capture --help` 明确：capture 读取 screen 和 scrollback；默认输出 text；支持显式 session/block、`--trim` 和 `--unwrap`；没有行数参数；输出总是以换行结束。terminal 的 scrollback 有官方容量上限。

宿主编排者在计划返回后补读了连接中的 revision 11 schema：

- `program_status.records[]` 的每条记录可以携带 `owner: {pid, name}`；owner 表示状态报告到达时的 terminal foreground process。
- `blocked` 本身表示 waiting for user，`kind` 只是 `permission | question | auth`。因此活动投影应对任何有效 `blocked` 记录映射 waiting_user，而不是只处理 `question`。

仍存在以下证据限制：

- `git status --short` 返回任务证据目录为 untracked。没有发现产品路径修改。不得将当前目录笼统称为完全干净。
- 仓库内缺少用户规则指定的 `sdk-skill-sources/appsdk-project-governance/references/ontology-governance.md`。本次读取了已安装副本。无需为本修复新建该缺失文件。

上述限制只影响 owner 解码的最终绑定和真实运行验收。其他设计、红测与实现可以按依赖推进。

## Ontology：对象、行为和关系

本节建模本次修复涉及的产品语义，不增加新的全局规则或审批设施。

| 对象 | 身份与属性 | 唯一 owner | 可观察入口 |
| --- | --- | --- | --- |
| Rex terminal endpoint | 现有 typed endpoint：server、service generation、session、block | 现有 binding owner | adaptor 调用参数及回执 |
| 通知输入 | 原始正文，以及本次提交追加的一个 CR | `collab/src/adapters/rex.rs` | fixture 中的 write JSON、字节回执、隔离 receiver |
| 临时 snapshot | 一次选定 endpoint 的屏幕/scrollback 文本；请求行数为 1..200 | Rex adaptor；server 负责鉴权和目标选择 | `worker snapshot` 返回值 |
| 程序状态记录 | state、kind、记录信息及 revision 11 声明的 owner | adaptor 解码；monitor 投影 | 官方查询、activity observation、公开 worker view |
| foreground process | 当前 pid/name | 官方 `terminal.process`；adaptor 承载 | observation |
| 通知拒绝文案 | 已选择 transport 和底层错误 | `notification_rejected_label` | 发送响应及现有错误记录 |

关系约束：

- Collab 普通身份与 Rex terminal endpoint 分离。本次不改变身份键或绑定流程。
- 一次通知输入对应一次有副作用的 write；service 查询不算输入提交。
- 一次完整字节回执只证明包含 CR 的输入已排队，不证明程序执行、接收或消费。
- 一次 snapshot 对应一次被准入的 endpoint 读取，不生成 activity、presence、消息或消费事实。
- program-status owner 只有同时匹配当前 foreground 的 pid 和 name，才可参与当前 activity 投影。
- foreground 匹配只用于排除过期状态，不能成为 Collab 身份证明。

执行者将本模型和最终字段绑定记录在现有任务证据中。不要新建 ontology registry、ledger 或长期状态缓存。

## DAG 修订

复用 `docs/dagpipe/collab-rex-adaptor.graph.json`。现图为单入口、单出口线性结构；本次没有新增拓扑的必要。

修订受影响节点的语义：

| 现有节点 | 必须修订的内容 | 实现或 gate |
| --- | --- | --- |
| 选择本次操作的权威输入 | 明确 snapshot 是既有 binding 上的观察请求，携带请求行数 | snapshot handler |
| 校验操作事实与候选 | snapshot 验证鉴权、目标 transport 和 `1..=200`；通知 mode 在副作用前验证 | handler、notify |
| 核实服务事实与完整目标 | 复用现有 service-generation 边界；不增加身份恢复或重复准入探针 | Rex adaptor |
| 执行该操作的最小 Rex 动作 | notify 一次 write `text + CR`；snapshot 只调用 capture；状态查询保留 owner | Rex adaptor |
| 分类结果输入阶段与观察来源 | 删除“write 后单独 Enter 接受”的错误合同；区分完整排队、明确拒绝、结果未知、capture 失败和状态查询缺口 | adaptor 错误与结果分类 |
| 提交所属既有 owner 的适用结果 | snapshot 不提交持久状态；activity 只提交本轮投影结果 | handler、monitor |
| 输出 Rex 操作回执 | capture 返回临时 snapshot；通知回执不冒充消费；失败保留 Rex 原错 | 公开响应 |

所有失败仍经现有单出口闭合。不得增加“查询失败后自动 write CR”“unknown 后换 transport”等补偿边。

图修订完成后执行一次：

```sh
dagpipe graph validate docs/dagpipe/collab-rex-adaptor.graph.json
dagpipe graph inspect docs/dagpipe/collab-rex-adaptor.graph.json
```

保存实际回执。静态通过只证明图结构，不证明行为通过。仅在现有映射确实引用变化符号时更新对应绑定，不新增无关 map。

## 通知提交：一次 write 包含正文和 CR

### 输入序列

`notify` 保留现有 mode 校验、endpoint 选择、service-generation 检查和同一绝对 deadline。

提交数据必须为：

```text
原始 text 的全部 UTF-8 字节 + 单个 0x0D
```

通过一次 `com.superlogical.terminal.write` 的 `args.data` 发送。该通知路径不得再调用 `send-key enter`。

这里的“一次”指一次有副作用的 terminal write，不要求删除必要的只读 service 查询。它也不表示 Rex 提供了程序执行事务。

正文原样保留：

- 不 trim。
- 不将正文内换行改为 CR。
- 不加入 bracketed-paste 或其他未经证实的输入包装。
- 不按字符数核对字节。
- 不删除正文已有的末尾字符。

预期字节数为 `text.as_bytes().len() + 1`。空正文也应排队一个 CR。

### Receipt 与失败语义

成功回执应明确包含：

- `transport: "rex"`；
- `bytes_written`，包含追加 CR；
- `text_queued: true`；
- 表示本次 CR 已随完整 write 排队的字段；
- `consumed: false`；
- 原 mode。

删除或更名当前 `enter_accepted` 字段，避免继续暗示发生过独立 Enter API 接受。优先使用 `submission_queued: true`，并在现有消费者和测试中完成一致更新。不得同时保留两个含义重叠的新旧字段。

错误分类：

| 结果 | 分类与处理 |
| --- | --- |
| mode、endpoint 或 service 前置检查明确失败 | KnownNotDelivered；不 write |
| write 返回已知 native rejection | 保留已知拒绝分类、native code 和原始 detail |
| write 超时、EOF、坏 JSON、缺字节确认或短写 | Unknown；不得补 Enter、重发或换 transport |
| 完整字节回执 | 输入排队成功；不提交接收、消费或回复事实 |

`RexInputStage` 只保留仍有真实消费者的阶段。若 `EnterAccepted` 已无使用者，删除该变体及相关描述。`WriteQueued` 记录已确认的实际排队字节，不能把它解释为程序已执行。

### 红测与反例

在旧实现上先证明：

1. `notify("中文\n第二行")` 应产生一次 write，JSON 中 data 精确等于正文加 `\r`；旧实现失败。
2. 成功回执应计入 CR 字节；旧实现失败。
3. fixture 将任何 `send-key` 调用设为失败；新的通知路径仍成功，且调用日志中没有 send-key。
4. 短写、坏回执或超时后，无任何第二次输入动作。
5. queued mode 仍在任何 spawn 前拒绝。

真实隔离 receiver 还须证明 CR 会让一次完整正文进入接收逻辑。仅检查 `bytes_written` 不足以证明这一点。

## Snapshot：官方 capture、有界返回、无输入副作用

### 唯一实现路径

在 `collab/src/adapters/rex.rs` 增加与现有调用设施一致的 snapshot primitive。`handle_worker_snapshot` 保留鉴权和目标选择，并按 transport 分派：

- Tmux：保留现有实现。
- Rex：调用新 Rex snapshot primitive。
- 其他 transport：继续返回其真实 unsupported 结果。

保持当前锁边界：在 state lock 内鉴权并 clone transport，释放锁后执行外部 capture。不得持有 reducer lock 等待 CLI。

使用固定官方 CLI、显式 server/session/block、`--autostart=false` 和既有有限 deadline。capture 使用 text 格式，并采用 `--unwrap --trim`，然后按逻辑文本行裁取最新行。

不得用 `terminal.write("\r")`、send-key、屏幕刷新输入或状态查询替代 capture。

### 输出合同

公开数据形状为：

```json
{
  "transport": "rex",
  "session_id": "<被读取的 Rex session>",
  "block_id": "<被读取的 Rex block>",
  "worker_id": "<目标 worker>",
  "lines": 20,
  "output": "<最多 20 行的最新文本>",
  "durable": false
}
```

`lines` 表示请求上限，保持现有 tmux 合同语义，不表示实际返回行数。不伪造 `pane_id`。

边界要求：

- `0` 和 `201` 在 capture 前失败。
- `1` 和 `200` 合法。
- 返回行数不超过请求值。
- 保留行顺序及正文中的空行。
- 只处理尾部空白展示行，不 trim 各行正文。
- 空 terminal 可成功返回空 output。
- 官方结尾换行不额外制造一条空记录。

官方 CLI 没有行数参数。不能发明 `--lines`。读取 stdout 时使用保留最新 N 行的缓冲方式，避免先把整个 scrollback 拼成一个大 String 再裁剪。完整耗时继续受同一 deadline 限制。不要为此引入新依赖、通用流框架或任意截断正文的字节限额。

### 副作用和失败终点

snapshot 不得：

- write 或 send-key；
- 改 activity、presence、identity、subscription；
- 提交消息接收、消费或 ACK；
- 保存屏幕内容到生产日志或新增持久快照；
- 因 capture 失败恢复身份或切换 transport。

服务代际不匹配、目标缺失、capture 非零退出、超时或读取失败均返回显式错误，保留原始错误依据。不得将失败变成成功空字符串，也不得返回未声明的部分输出。

红测覆盖官方 argv、显式目标、行数边界、最新行裁取、Unicode、空输出、尾部换行，以及失败时无 write/send-key。

## Rex 通知拒绝标签

唯一映射 owner 是：

```text
collab/src/server/mod_parts/part_05.rs
notification_rejected_label
```

增加：

```text
"rex" -> REX_NOTIFICATION_REJECTED: <原始错误>
```

不得在 adapter、各调用点或 CLI formatter 再包一套相同标签。底层 `REX_NOT_DELIVERED:`、`REX_UNKNOWN:`、operation、native code 和 detail 原样保留。

测试同时断言：

- Rex 明确拒绝和未知结果均使用 Rex 外层标签。
- 不出现 `TMUX_NOTIFICATION_REJECTED:` 或其他 transport 标签。
- 底层真实分类不因外层文案变化而改变。
- 现有 tmux、DSH、Desktop、AppServer 映射保持原结果。

本次不追查没有当前源码证据的 `codex bridge` 文案来源。

## 状态投影：先过滤 owner，再聚合有效记录

### 解码前置

从宿主现有 revision 11 schema 补入 owner 的准确 JSON 位置和示例。只补这一事实，不重新做环境或身份探索。

adapter 必须保留完整 owner pid/name。禁止只保留 pid，也禁止从 `app`、记录 ID、cwd 或日志推测 owner。若 owner 属于响应整体，则在响应层承载；若属于各记录，则在记录层承载。不得同时支持未经 schema 证实的两种形状。

### 优先级

使用以下固定顺序：

1. 必需的 process 或 program-status 查询失败、解析失败或数据不完整：投影 Unknown。
2. owner 无法完整核实：对应状态不参与 Busy 或 WaitingUser 判定。
3. owner 的 pid 或 name 与当前 foreground 不同：丢弃该过期状态。
4. 对全部有效记录聚合：`blocked`（任意 kind）优先于 `working`。
5. 没有等待用户或工作中的有效记录时，按下表投影。

| 有效输入 | activity | 公开 `thread_state` |
| --- | --- | --- |
| 任意有效 `blocked` | WaitingUser | `waiting_user` |
| 无上述记录，存在有效 `working` | Busy | `working` |
| 有效 `idle`、`done`、`error` | Idle | `idle` |
| 有效但未知或缺省 state | Idle，按用户提供的 Rex API 语义 | `idle` |
| 成功完整查询但没有状态记录，或记录全部过期 | Unknown | `unknown` |
| 必需查询失败或 owner 证据不足 | Unknown | `unknown` |

“未知 state 按 idle”只适用于已获得并通过 owner 过滤的记录。它不等于“查询失败按 idle”。

多条记录不再一律作为冲突。不得按返回顺序挑第一条，也不按 `updated_at` 自行选择“最新状态”。本次不新增 TTL、时间窗口或状态历史。

过期 working 不能覆盖当前 idle。过期 question 也不能覆盖当前 working。pid 相同但 name 不同必须判为不匹配。

### 事件、错误与状态提交

复用当前 monitor 和现有事件流。`program_status_changed`、`program_status_removed`、`process_changed`、`child_exited` 触发当前快照重读。事件 payload 只负责触发，不直接成为最终 activity。

继续保留：

- 旧 binding 迟到结果不能提交到新 binding。
- 流断开或必需观察失败时用 Unknown 覆盖旧 Busy。
- 程序状态不证明 agent 在线。
- 程序状态不证明消息接收、消费、回复或任务完成。
- `done` 只投影非 working，不提交 board 完成。
- `error` 只投影非 working，并在现有 reason 中保留来源；不伪造任务失败。

不要在本次修改后台属性、身份恢复或任务生命周期策略。

## 现有测试的改写与删除

| 现有测试或 fixture | 动作 |
| --- | --- |
| `notify_deadline_retains_the_queued_write_and_skips_enter` | 改写为单次提交共用 deadline；删除依赖第二次 service 检查超时的合同 |
| `enter_failure_preserves_write_queued_stage` | 删除 notify 的分阶段 Enter 合同；若独立 `send_enter` 仍有实际消费者，只保留该 primitive 的独立测试 |
| `typed_native_rejections_keep_exact_code` | 保留 write native rejection；删除 notify 必经独立 Enter 的断言 |
| `write_unknown_and_short_write_never_enter` | 改为未知后不再产生任何输入动作，并检查包含 CR 的预期长度 |
| `allowed_modes_return_rex_receipt_json` | 更新 CR 字节数及新的排队回执字段 |
| `status_mapping_covers_working_blocked_and_unknown` | 加入真实 owner/foreground；替换多记录一律 Unknown、done/error 一律 Unknown 的旧断言 |
| `projection_keeps_source_and_never_emits_idle` | 删除“永不 idle”合同，改为保留来源并正确投影 Idle |
| `observe_current_preserves_sources_without_inferring_idle` | 保留 primitive 不做 activity 推断的边界；补 owner 无损解码断言 |
| `fixture_rex.py` | 增加 capture 行为与 owner 数据；移除只服务于已删除 notify Enter 合同的场景 |

保留现有目标精确匹配、service generation、deadline、owned child 收口、事件过滤和旧 binding 隔离测试。不要以消融为由删除这些独立保障。

不要直接运行现有 `rex_public_contract` 全流程。该流程使用 context、接收和生命周期动作，与本合同限制冲突。应复用其中不触发这些动作的辅助设施，建立本次聚焦用例。

## 执行批次与验证

### 第一批：设计和红测

执行 owner 先回看任务笔记，确认 worktree、分支和当前 dirty 内容归属。将本计划保存到约定路径，并在现有笔记关联。

随后：

1. 补 owner schema 证据。
2. 修订 ontology 绑定和现有 DAG。
3. 执行图 validate/inspect。
4. 添加或改写上述行为测试。
5. 在未修改产品实现时运行对应测试，保留真实失败断言。

红测必须是行为失败，不能用编译失败、空过滤或 fixture 启动失败代替。

### 第二批：最小实现

按唯一 owner 修改：

1. Rex adapter：单次通知输入、capture primitive、owner 解码。
2. Rex monitor：owner 过滤和记录聚合。
3. snapshot handler：增加 Rex 分派，保持锁边界。
4. notification label owner：增加 Rex 标签。
5. 更新直接受影响的测试和合同描述。

同一 `rex.rs` 由一个 writer 负责。不要为四项修复新增 transport 抽象或公共框架。

### 第三批：聚焦验证

从实际 Cargo target 确认测试路径。主要入口为对应 binary 的：

- `adapters::rex::tests`；
- `server::rex_monitor::tests`；
- 新增的 snapshot handler 定向用例；
- notification label 定向用例。

先用 `--list` 核对过滤结果。实际执行不得为零。运行对应 manifest 的格式检查：

```sh
cargo fmt --manifest-path collab/Cargo.toml -- --check
```

不运行 `cargo test --all`、广谱 mailbox 测试或整个旧 Rex live 合同。

### 第四批：真实入口 black-box

分成两层取证：

| 层级 | 用例 | 证明范围 |
| --- | --- | --- |
| 隔离 fixture 的公开请求入口 | 鉴权后的 worker snapshot、通知失败响应、公开 worker 状态 view | server 分派、返回合同、错误标签和状态展示映射 |
| 显式隔离 Rex target | 单次正文加 CR、官方 capture、当前/过期 owner 状态切换 | 官方 CLI 与真实 terminal 行为 |

隔离 Rex 用例必须证明：

- receiver 实际收到一次完整输入，未依赖独立 send-key；
- snapshot 在不发送任何输入的情况下读取已输出标记；
- 返回最新 N 行，且不混入其他 block 的标记；
- working 可切换为 idle、done、error、未知 state 的非 working 展示；
- foreground 更换后，旧 owner 的 working 不再显示；
- waiting_user 对有效 working 的优先级符合计划。

测试员须在真实 Rex 前端观察隔离目标，并保留可复现步骤及结果。fixture、源码和单测不能替代此项验收。

本合同不允许为完成验收启动生产 daemon、修改真实 identity-map 或注册测试身份。若公开端到端入口必须依赖尚未获准的运行环境，完成其余验证后报告该具体缺口。不得扩大权限，也不得将直接 adapter 探针冒称完整 Collab live 验收。

## Review、提交、PR 和合并

完整 milestone 的自检、受影响测试、适用黑盒和真实前端验收完成后，再启动一次独立 review。

Reviewer 必须亲读：

- 当前有效四项需求；
- 本计划与任务笔记；
- 修订后的 DAG；
- 完整变更；
- 红测、聚焦回归、black-box 和前端验收证据。

审查重点：

- 单次输入是否真包含 CR，是否残留第二次输入路径；
- receipt 是否把排队冒称消费；
- snapshot 是否有界、只读且在锁外执行；
- native 原错是否保留，Rex 标签是否由唯一 owner 生成；
- owner 是否同时比较 pid/name；
- 多记录优先级和状态缺失边界是否正确；
- 旧错误测试合同是否已移除；
- 是否触及身份、订阅或其他 worktree；
- 是否存在未获证据支持的完成宣称。

Finding 修复后只重跑受影响验证，并复核 finding。不重复启动完整 review。

独立 PASS 后，由宿主执行 owner：

1. 核对最新 `origin/main` 与受影响差异；组合候选后只补必要验证。
2. 确认 worktree 数量符合现有提交 gate。
3. 提交完整 milestone，创建 PR。
4. 等待适用 CI，通过后按既有授权合并到 `origin/main`。
5. 核对 PR 已合并及远端主线结果。
6. 在已交付位置保留必要计划和行为证据。
7. 移除本任务 worktree、分支及已确认可删除的临时资源。

不得绕过 hook、强推、覆盖他人 dirty 内容或清理其他 owner 的资源。冲突、push 拒绝和 CI 失败只暂停受影响集成链，保留原错并修复。

## 资源收口与完成标准

只清理有明确 owner 和标识的资源：

- 测试创建的具名 Rex session；
- 本任务 child PID；
- 本任务 fixture、临时构建及证据目录中不再需要的内容；
- 已交付且无未交付内容的本任务 worktree。

不执行批量 session termination、broad process kill 或任何 `tmux kill-server`。不关闭当前工作宿主来完成测试清理。

完成回执须分别说明：

- 四项用户行为的真实验收结果；
- 独立 review 结果；
- PR 和远端主线交付位置；
- 自有资源实际清理结果；
- 全局安装和生产 live 状态。

只有代码、测试和 review 通过时，不能宣称生产运行态已修复。若真实前端或公开入口验收缺失，milestone 保持 `INCOMPLETE/UNVERIFIED`，不进入放行。

历史记忆曾记录“write 后独立 Enter”的旧合同。本计划已用当前任务笔记、源码和用户提供的 revision 11 事实取代该合同；未将旧记忆作为当前行为依据。
