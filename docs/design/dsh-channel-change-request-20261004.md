# DSH 通道接入：对 appsdk 侧的变更请求

- **来源**：dsh-gateway（本侧仓库 `/Volumes/extension/code/dsh-plugins/dsh-gateway`）
- **审查对象**：appsdk `main` @ **`9a9d674`** —— **本文件所有行号均在该 SHA 实测**（审计始于 `7a23f53`；`git diff 7a23f53..9a9d674 -- collab/` 为空，故两 SHA 下 `collab/**` 逐字节相同，行号通用）
- **对端冻结契约**：`COLLAB-CHANNEL-INTERFACE.md` **v10.1.9**（本侧仓库 `docs/`）+ 设计稿 `COLLAB-DSH-CHANNEL-DESIGN.md`
- **关系**：appsdk 已有的 `docs/dagpipe/collab-dsh-channel.graph.json` 描述对端侧的同一条对象流，本文件是它的**变更面清单**

---

## 0. 一句话结论

本侧（dsh-gateway）已按"**被动通道**"把契约冻结并实现完毕。**appsdk 侧只剩 1 处必须改（S27，阻塞"恰好一次"）+ 3 处语义正确性缺口（S22/S23/S25）**；其余 S 项已落地或已核对为无需改动（见 §5 的逐条核对结论，含一处**请勿改**的契约）。

---

## 1. 背景：我们要一起做成什么

- **目标**：DSH 的 agent 能注册进 collab，并能**双向通信**。
- **本侧定位（用户裁决，硬约束）**：gateway 只做**被动传输**——排队 / 投递 / 回执 / 投影。**不解释内容、不生成回复、不决定收件人**。
- **入站模型 = R-b（用户裁决）**：**collab 主动 push**，调本侧控制面 op `enqueue` 并**携带正文**。本侧**不再有** drain 循环、`Req::Inbox` 拉取、`channel-wake`、`channel-presence`、`--collab-drain-interval-ms`。
- **出站**：本侧 daemon 以对应 peer 身份调 collab 的 `Req::Send`（构造合法 `CommandEnvelope`），正文原样透传。

---

## 2. 边界：谁负责什么

| | 本侧（dsh-gateway） | 对侧（appsdk / collab） |
|---|---|---|
| 控制面 | unix socket 上的 NDJSON 请求/响应（`agent-facts`/`enqueue`/`channel-redrive` 等） | 调用方 |
| 去重锚点 | 按调用方给的 `messageId` 去重（`foldRecorded`） | **`messageId` 的生产者**（S27） |
| 内容 | 原样透传，不解释 | 决定 `from`/`to`/正文 |
| 存活判定 | 把 `status` 投影为 `running \| inactive` | 比较字面量 `"running"`（已对齐，见 §5） |
| collab 协议 | **不直接实现**；由本侧 daemon 的 collab 代理（B 段）代跑 | 真源 |

---

## 3. 【阻塞】S27：`enqueue` 请求必须携带 `messageId`

### 3.1 现状（一手证据，`9a9d674`）

| 位置 | 事实 |
|---|---|
| `collab/src/server/mod_parts/part_01.rs:195` | dsh sink 分支**已经持有** `message_id`：`let (gateway_mode, internal_marker) = dsh_wake_mode(mode, message_id)?;` |
| `collab/src/server/mod_parts/part_01.rs:196-203` | 但调用 `dsh::notify(endpoint, runtime_id, agent_id, gateway_mode, source_thread_id, body)` —— **没有把 `message_id` 传下去** |
| `collab/src/adapters/dsh.rs:283-290` | `notify` 签名里**没有** `messageId` 形参 |
| `collab/src/adapters/dsh.rs:295-301` | 请求体只有 `runtimeId` / `agentId` / `mode` / `content` / `sender` |
| `collab/src/server/mod_parts/part_01.rs:166-171` | **对照**：AppServer sink 已经把它的 id 传下去了（`client_user_message_id`）⇒ **只有 dsh 这一条路径丢掉了它** |

### 3.2 改什么（改动量：一个形参 + 一个 JSON 字段）

**(a) `collab/src/adapters/dsh.rs`** —— `notify` 增形参并写进请求体：

```rust
pub fn notify(
    endpoint: &str,
    runtime_id: &str,
    agent_id: &str,
    mode: &str,
    sender_id: &str,
    text: &str,
    message_id: &str,          // ← 新增
) -> Result<serde_json::Value, ControlError> {
    let socket = control_socket(endpoint)?;
    let result = round_trip(
        &socket,
        "enqueue",
        serde_json::json!({
            "runtimeId": runtime_id,
            "agentId": agent_id,
            "mode": mode,
            "content": [{"type": "text", "text": text}],
            "sender": {"runtimeId": "collab", "id": sender_id, "name": "collab"},
            "messageId": message_id,   // ← 新增
        }),
    )?;
    // …其余不变（仍读回 result.messageId）
}
```

**(b) `collab/src/server/mod_parts/part_01.rs:196`** —— 把已有的 `message_id` 传进去：

```rust
let mut receipt = crate::client::adapters::dsh::notify(
    endpoint, runtime_id, agent_id, gateway_mode,
    source_thread_id.unwrap_or("collab"), body,
    message_id,                     // ← 新增（该变量在 :195 已在作用域内）
)
```

**(c) 取值规则（本侧契约 §3.2，算法与长度都定死）**：

```
messageId = "collab:" + sha256( collab 状态目录的规范化绝对路径 )[..16] + ":" + msg_id
```

- 状态目录 = `$COLLAB_STATE_DIR`，否则 `$HOME/.collab`（**对端持有，本侧不持有**）。
- `msg_id` = collab 自己的 `gen_msg_id()`。
- 摘要**必须**是 `sha256` 的**前 16 个十六进制字符**（64 bit）：
  - 短于 64 bit 会重新引入撞号（生日界），而撞号在"first enqueue wins"下会**静默吞掉第二条消息**；
  - 摘要**只依赖状态目录**，不引入任何新配置。
- 本侧只做**形状校验**：必须匹配 `^collab:[0-9a-f]{16}:m[0-9]+-[0-9]+$`，不匹配 ⇒ 显式返回 `invalid-message-id`（**不降级为告警**，因为形状错＝忘了命名空间＝静默丢消息）。

> **文档一致性提示（以契约正文为准）**：契约正文 §3.2（两处）规定摘要输入是 **collab 状态目录**；本侧仓库的 §22 历史修订块里有一处写成了"socket 绝对路径"。**以正文的状态目录为准。** 本侧只校验形状、无法验证摘要输入，所以这一条不影响互操作，但影响"跨实例不撞号"的性质，请按正文实现并在你的测试里定死。

### 3.3 为什么必须改（后果是确定的，且本侧无法自行修复）

1. collab **确实会重放唤醒**：`part_01.rs:107-108` 定义 `daemon-live-closure` / `restart-replay-pending` 两个 mode 常量，`:232-250` 的 `dsh_wake_mode` 把它们映射为 `followup` + `internal_marker=true`，生产发出点在 `part_08.rs:602-606`。
2. 重放时如果本侧拿不到对端的 `msg_id`，只能自己 mint `randomUUID()`。
3. 本侧按 `messageId` 去重（`foldRecorded`）⇒ **新 id 认不出这是同一条** ⇒ **同一条消息被投递两次，agent 会看到两次**。

**用户已裁决 R-b**（本侧接受对端的推送模型），因此**去重的唯一责任点就是把对端的 `msg_id` 传下来**——本侧不能凭空造出对端的 `msg_id`。**若暂不落地**：通道仍可工作，但语义降级为 **at-least-once**，且重复是**对 agent 可见的重复投递**，不是内部重试。

### 3.4 验收证据（怎么证明改对了）

- **单测**：`collab/src/server/dsh_channel_tests.rs:633` `dsh_wake_enqueue_maps_the_mode_and_returns_a_queue_receipt` 增加一条断言：
  ```rust
  assert!(requests[0]["params"]["messageId"]
      .as_str().unwrap()
      .starts_with("collab:"));
  ```
  更严格可断言正则 `^collab:[0-9a-f]{16}:m[0-9]+-[0-9]+$`。
- **行为**：同一 `msg_id` 重放两次 ⇒ 本侧只入队一次（第二次 `enqueue` 返回 `already-pending` / `already-settled` 且**不 append**）。
- **不会破坏现有断言**：`:660-664` 是**逐字段**断言（不是整对象相等），`:1005` 的"每次 send 恰好一次 `enqueue`"不受影响。

---

## 4. 【语义正确性】S22 / S23 / S25

这三项不阻塞通道打通，但会让 dsh 通道在生产上**给出错误语义**（`cargo test` 不会报错）。

### S22 — 兜底 runtime 标签把 dsh 混进 appserver 命名空间

- **位置**：`collab/src/server/mod_parts/part_02.rs:109`
- **现状**：`_ => format!("runtime-{}-appserver", transport.kind.as_str())` ⇒ dsh 传输被标成 **`runtime-dsh-appserver`**。
- **要改**：兜底标签必须按 kind 生成正确的名字（不要复用 `-appserver` 后缀），否则运维看到的 runtime 名字会指向一个不存在的 AppServer 传输。

### S23 — `same_pane_tmux_recovery` 对 dsh 恒 `false`

- **位置**：
  - `collab/src/server/mod_parts/part_02.rs:238`（判定）、`:301`（作用点：**不重设 current-thread route**）
  - `collab/src/server/mod_parts/part_06.rs:1116`（判定）、`:1133`（`is_master_binding && !same_pane_tmux_recovery`）、`:1138`（产出 **`MASTER_RECOVERY_BLOCKED_LIVE`**）
- **现状**：该判定要求新旧传输都是 `Tmux` 且同 pane ⇒ dsh 恒 `false` ⇒ master 重连时被判成"live master 仍在，禁止自动恢复"。
- **要改**：dsh 需要一条**与 tmux pane 无关**的等价恢复判定，或显式声明 dsh 不走该路径。
- **⚠️ 不要顺手改错**：同文件 `part_02.rs:262-282` 的 `reissued_master_grant` 是**传输无关**的，dsh 的 master grant **会被正常重发**——不要把它一起改了。

### S25 — rebind 拒绝文案硬编码 tmux / AppServer

- **位置**：`collab/src/server/mod_parts/part_10.rs:768`、`:772`（`"RUNTIME_BINDING_REJECTED: CLI rebind requires the current tmux pane"`）、`:686`（`"TRANSPORT_UNSUPPORTED: App Server rebind is retired; register the current tmux pane"`）
- **现状**：dsh 的 rebind 被拒时会收到**指向 tmux 的误导文案**，把排障方向带偏。
- **要改**：文案按 kind 分支（或改为传输无关的表述）。

---

## 4.3 【阻塞】S31：notification-batch 把合成 wake id 当 mailbox `msg_id` 传给 dsh sink

**现象（读码 + 一手日志）**：`part_05.rs:789` 在投递 notification-batch 时这样调用：

```rust
deliver(transport, source_thread_id, &text,
        &format!("collab-notification-{}", first.1),   // ← 合成 wake id
        explicit, &delivery_mode)
```

`deliver` 把第 4 个参数**原样**当作 `message_id` 传给 dsh sink（`part_01.rs:196`），
sink 再用它构造入站 `messageId`（`dsh.rs:331` → `wake_message_id`，`dsh.rs:293-306`）：

```rust
Ok(format!("collab:{prefix}:{msg_id}"))
```

⇒ 本侧实际收到 `messageId = collab:<digest>:collab-notification-m1791124915946-13`，
**不是** `collab:<digest>:m1791124915946-13`。

但 `wake_message_id` 的文档注释（`dsh.rs:281-292`）声明该 id 必须**派生自 collab 自己的 `msg_id`**，
且本侧网关"校验形如 `^collab:[0-9a-f]{16}:m[0-9]+-[0-9]+$`"。
notification-batch 路径**违反了自己的契约**。

### 为什么必须改

本侧收到后回 `Req::Ack` 时只能剥离 `collab:<digest>:`，于是提交
`collab-notification-m1791124915946-13`。而 `part_11.rs:179-256` 的 Ack 按**真实 mailbox id**
在 `st.msgs` 与 `.agent-collab/mailbox/<id>.json` 里查找 ⇒ 落入 `not_found`，
但响应仍是 `Resp::data(json!({acked, already_acked, not_found}))`，**`ok:true`**。

⇒ **通知回执变成"成功响应下的空操作"**：对端永不认为该消息被消费，本侧也无法察觉。
本侧已按 review 要求显式暴露 `not_found`，但**根因在对端**：mailbox id 在传参时就被丢了。

### 改什么

`part_05.rs:789` 的 notification-batch 路径把**真实 mailbox id**（`first.1`）传给 `deliver` 的
`message_id` 形参；合成的 `collab-notification-<id>` 只用于**投递/日志**标识，不进入 mailbox 语义。

若确实需要区分"投递 id"与"mailbox id"，请在 typed 契约中把两者**分别显式命名**，
不要复用同一个形参。

### 验收证据

1. 触发一次 notification-batch 唤醒 ⇒ 本侧收到的 `messageId` 形如
   `collab:<digest>:m<ms>-<seq>`，**不含** `collab-notification-` 前缀。
2. 本侧就该 id 发 `Ack` ⇒ 响应中该 id 出现在 `acked`（或 `already_acked`），**不在** `not_found`。
3. 对端 mailbox 中该消息状态从 `delivered` 变为已消费。

**本侧对应证据**：`packages/daemon/tests/collab.test.ts` 的
`an ack the peer did not match is reported instead of passing as consumed`
（用对端真实 Ack 语义建模：`not_found` ⇒ 本侧显式报错）。

---

## 4.2 【阻塞】S30：重注册必须更新该 worker 的 transport

**现象（一手证据，真实对端联调）**：本侧用**同一 worker id、同一 token、同一状态目录**注册两次
（daemon 在两次之间重启，DSH runtime id 从 `rt-d40dc6d8-…` 变为 `rt-5f25a52c-…`）。

- 第二次 `Register` **返回 `ok:true`**（本侧因此认为恢复成功）；
- 但对端项目 journal `.agent-collab/server/journal.jsonl` 里，该 worker 的
  **`Registered` 事件只有 1 条**，且 `transport.namespace` 仍是**第一次**的 `rt-d40dc6d8-…`：

```json
{"ev": "Registered", "worker": {"id": "dsh-dshgw-tui-e2e4", "token": "a399740f…",
 "cwd": "/private/tmp/dshgw-probe/tui-project", "registered_ms": 1791126966158,
 "transport": {"kind": "dsh",
   "endpoint": "unix:///var/folders/…/dshgw-restart-tui-fqPM/control.sock",
   "namespace": "rt-d40dc6d8-3245-45db-8471-566e749a1c08",
   "session_id": "dshgw-tui-e2e4", "thread_id": "dshgw-tui-e2e4",
   "capabilities": ["enqueue_wake","agent_facts"]}}}
```

- 对端随后把回信推到**旧的** `rt-d40dc6d8-…`，本侧网关如实拒绝：

```
collab: DSH_NOTIFICATION_REJECTED: DSH_ENDPOINT_REJECTED:
gateway refused enqueue with unknown-runtime:
no connected runtime rt-d40dc6d8-3245-45db-8471-566e749a1c08
```

### 为什么必须改

⇒ **重启后该 agent 能发、不能收**。`Send` 成功，但任何回信都发往一个已死的 runtime。

**本侧无法自行修复**：`Register` 每次都携带**新的** `candidates.dsh.runtime_id`，但对端对已有 worker
保留了旧 transport；现有 op 表里**没有任何 op 能更新 transport**。

### 改什么

重注册一个**已存在**的 worker（同 worker id + 同 token）时，用本次 `candidates.dsh` **替换**其
`transport`（`endpoint` / `namespace`(runtime_id) / `session_id` / `thread_id` / `capabilities`），
并写一条新的 `Registered` 事件；**不要**静默保留旧值。

若这是有意为之（把重注册设计成幂等空操作），则请提供一个**显式 op**（例如
`RebindTransport` / `Register{replace_transport:true}`），让本侧能表达"这个 worker 换了 runtime"。

### 验收证据

1. 用 runtime `R1` 注册 worker `W` ⇒ journal 的 `Registered.transport.namespace == R1`。
2. 停掉本侧 daemon，用 `R2`（`R2 != R1`）以**同一 worker id + 同一 token** 重注册 ⇒ `ok:true`，
   且 journal **新增**一条 `Registered`，其 `transport.namespace == R2`。
3. 向 `W` 发一条消息 ⇒ 对端把通知推到 `R2` 的 control socket，本侧收到并唤醒 agent。

**本侧对应证据**：`docs/notes/review-evidence/b-stage-collab-tui/peer-registered-record.jsonl`
（2 次注册只有 1 条 `Registered`）、`tui-reply-rejected.txt`（对端如实上报投递失败）、
`restart-tui-run2.txt`（第二次注册 `ok:true` 且出站成功）。

---

## 4.1 【阻塞】S29：`is_definitely_absent()` 只认 `unknown-agent`

**契约已改**：本侧 `COLLAB-CHANNEL-INTERFACE.md` **v10.1.11** §3.1 新增消融项 **N26**。

### 现状（对端）

`dsh.rs:62-67`：

```rust
fn is_definitely_absent(&self) -> bool {
    matches!(self, Rejected { code, .. } if code == "unknown-runtime" || code == "unknown-agent")
}
```

`probe()` 只在它返回 `true` 时判 `Absent`，而 `identity.rs:1234-1245` 的退休路径会**删除 route**。

### 为什么必须改

`unknown-runtime` 在本侧的真实语义是"**本网关现在观测不到这个 agent**"，不是"该 agent 不存在"。触发它的情形包括：runtime 未连接、`agent/get` 超时、runtime 中途掉线、回复畸形、回复缺 `agent`/`cwd`。

⇒ **一次瞬时故障就会让对端退休一个活着的 peer，并删除 route，不可逆。**

### 改什么（改动量：一个谓词）

```rust
// 只有肯定的"本网关不认识这个 agent"才退休。
fn is_definitely_absent(&self) -> bool {
    matches!(self, Rejected { code, .. } if code == "unknown-agent")
}
```

**本侧同步义务（已实现）**：`unknown-agent` **只在**网关 `agent/get` 明确回 `AGENT_NOT_FOUND`（`-32003`）时返回；其余一切失败返回 `unknown-runtime`。

**为何不加新码**：§3.1 词表封闭，新增码需双方同时升级；改判定含义是零新增词汇的最小改法。

### 验收证据

对端补一条用例：构造 `Rejected{code:"unknown-runtime"}` ⇒ `is_definitely_absent()` 为 `false`、`probe()` 为 `Unknown`、**不退休**；构造 `Rejected{code:"unknown-agent"}` ⇒ `true`、`Absent`、退休。

本侧对应回归：`packages/daemon/tests/control.test.ts` 的
`agent-facts keeps "gone" and "cannot tell" as distinct error codes`（三段：`AGENT_NOT_FOUND` → `unknown-agent`；`CAPABILITY_MISSING` → `persistence-unavailable`；其它失败 → `unknown-runtime` 且 **≠ `unknown-agent`**）。

---

## 5. 已核对为「无需改动」/「已落地」——请勿回退

以下均在本文件基线上**实测**过，结论是"已经正确"：

| 项 | 结论与证据 |
|---|---|
| **S1–S3** | 已落地：`TransportKind::Dsh`、`DshCandidate`、`admit_dsh_candidate` 均存在 |
| **S5** | 已落地：`dsh_wake_mode`（`part_01.rs:232-250`）穷举 5 值域，含两个内部标记 |
| **S18** | 已落地：订阅匹配不再硬编码 `"appserver" \| "tmux"`，改用 `TransportKind::from_method(...)` ⇒ `"dsh"` 可解析 |
| **S19** | 已落地：dsh 的 `thread_id` 为 `Some`（= agent id） |
| **S20** | 已落地：`dsh-wake-enqueued` / `DSH_NOTIFICATION_REJECTED` / 三变体传输标签穷举 |
| **S21** | **已满足，无需改动**（详见下方） |
| **S24** | **已关闭**：presence 已按 kind 分派（`part_07.rs:38` `TransportKind::Dsh => dsh_identity_presence`），dsh worker 不再被 tmux 专用扫描排除 |
| **S26** | 已落地：`SelectedTransport` 复用既有字段（`endpoint` / `namespace` / `thread_id`），**未新增 `runtime_id`** |
| **N25（请勿改）** | `part_07.rs:59` 与 `:223` 比较字面量 `"running"`。**本侧据此把 `status` 投影为 `running \| inactive`（由 `AgentView.live` 推导，不是直通 `AgentView.status`）。这是双方已对齐的契约**；若要改判定方式，请先回本侧改契约 |

### S21 为什么"已满足"（供 review 复核）

本侧契约 §10 要求：`ok:false` + 五个 dsh pre-delivery 码（`unknown-runtime` / `unknown-agent` / `invalid-message-id` / `bad-request` / `message-suppressed-dead`）必须落 **`KnownNotDelivered`**；而超时 / 连接失败 / 畸形响应必须落 **`Unknown`**（**不得重发**，因为消息可能已经入队）。

实测满足：

- `collab/src/adapters/dsh.rs:78-83` 的 `Display`：**任何** `ok:false` ⇒ `DSH_ENDPOINT_REJECTED: …`；`Unusable`（超时/畸形）⇒ `DSH_ENDPOINT_UNKNOWN: …`；`Unreachable` ⇒ `DSH_ENDPOINT_BLOCKED: …`
- `collab/src/adapters/mod.rs:209-222` 的 `KNOWN_NOT_DELIVERED_PREFIXES` 含 `DSH_ENDPOINT_REJECTED:` 与 `DSH_ENDPOINT_BLOCKED:`，**且刻意不含** `DSH_ENDPOINT_UNKNOWN:`（`:218-220` 有注释说明）
- ⇒ 五个码全部落 `KnownNotDelivered`；超时/畸形落 `Unknown` ⇒ **不重发**。语义正确。

---

## 6. 本侧已完成的对应工作（对端的前置，已交付）

A 段 `AgentView.scope.cwd` —— 因为对端 `AgentFacts.cwd` 是**必需字段**（`dsh.rs:264` 的 `field("cwd")?`），且 `admit_dsh_candidate` 用它 canonicalize 后与 project root 比对（`part_02.rs:1225-1236`），所以这是**对端已落地代码的硬前置**。

- **已实现并合并**：live 取 `agent.session.header.cwd`，cold 取持久化快照的 `header.cwd`（缺失时**省略** `scope` 字段）。
- **证据**：本侧 e2e 全量 **9 PASS / 0 FAIL**（候选 `1ef80d5`）；另有独立 live 探针断言 **live 与 cold 两条路径返回同一个项目根**，且该路径**绝对、存在、可 canonicalize** ⇒ 对端的 `fs::canonicalize(cwd) == candidate_root` 成立。
- **一个需要你知道的行为**：本侧返回的是 session **记录的原值**，**不做 canonicalize**。对端自己会 canonicalize，所以 macOS 上 `/tmp → /private/tmp` 这类符号链接不影响判定。

---

## 7. 实现状态：对端现在可以调通

- **本侧 B 段已落地**（2026-10-04）：`agent-facts` op 已实现，collab 客户端
  （`Register`/`Ack`/`Send`/`Context`/`MsgStatus`）、`channel-redrive`、插件发信工具
  `collab_send` 与 `collab/send` wire 方法均已实现，并完成真实对端联调（注册、
  双向收发、重启后重注册恢复）。
  ⇒ **对端现在可以调通。**（本节原为"尚未实现"清单，该描述已过期。）
- **仍未落地**：无。§4.1 的 **S29** 已于 2026-10-04 落地（见 §10.1）。
- 其余（S22/S23/S25/S27/S29/S30/S31）状态见 §4、§8 与 §10。

**对端调用 `agent-facts` 的形状**（本侧已按此实现）：

```jsonc
// 请求（NDJSON，一行）
{"method":"agent-facts","params":{"nonce":"<challenge>","runtimeId":"<rt>","agentId":"<session>"}}

// 期望的 result（nonce 必须回显；AgentFacts 的五个字段都必需且均为字符串）
{"ok":true,"result":{"nonce":"<回显>","runtimeId":"…","agentId":"…","sessionId":"…","cwd":"…","status":"running|inactive"}}
```

- `nonce` **必须原样回显**（对端在 `dsh.rs:244-252` 校验，不回显 ⇒ `challenge-mismatch`）。
- **缺席必须走错误码**，且**只有 `unknown-agent` 算确定缺席**（对端据此判 `Absent`），**不得**用 `status` 表达缺席。**`unknown-runtime` 不再是确定缺席**（见 §4.1 S29）。
- **无法判定**（持久化能力缺席、超时、runtime 中途掉线、回复畸形）走 `persistence-unavailable` / `unknown-runtime` ⇒ 对端判 `Unknown`，**绝不退休**。
- `status` 取值域是 **`running` | `inactive`**，由 `AgentView.live` 推导（见 §5 的 N25）。

---

## 8. 请对端做的事（checklist）

- [x] **S31（阻塞）**：`part_05.rs:789` 把真实 mailbox id 传给 `deliver` 的 `message_id`。**已落地**，见 §10.3
- [x] **S30（阻塞）**：重注册已存在的 worker 时**更新其 transport**。**已落地**，见 §10.2（真因是 `command_id` 去重，非"保留旧 transport"）
- [x] **S29（阻塞）**：`dsh.rs` 的 `is_definitely_absent()` 收窄为只认 `unknown-agent`。**已落地**，见 §10.1
- [x] **S27**：`notify` 增 `messageId` 形参 + 写进 `enqueue` 请求体。**上一轮已落地并合并**
- [x] **S22**：兜底 runtime 标签不再产出 `runtime-dsh-appserver`。**上一轮已落地并合并**
- [~] **S23**：**结论已修订**——原论证在改动前不成立（已有反证用例钉住）；S30 落地后该栅栏对 dsh 变为可达，故补了 dsh 同主恢复判定。见 §10.2
- [x] **S25**：rebind 文案不再对 dsh 指向 tmux。**上一轮已落地并合并**
- [x] **复核 §5**：已复核；`N25` 的字面量 `"running"` 契约**保持不变**

---

## 9. 引用基线

- 本文件所有 appsdk 行号：**`9a9d674`** 实测（该 SHA 相对审计起点 `7a23f53` 只动了 `rust/`、`contracts/`、`docs/` 与证据，**`collab/` 未变**）。
  > 实施前请按**当时 HEAD** 重核一次行号——本文件的判定依据是**谓词与行为**，行号只是定位手段。
- 本侧契约：`COLLAB-CHANNEL-INTERFACE.md` **v10.1.11**（含 §9 的 S1–S27 完整审计账与 §4.1 的 S29/N26；本文件是它的**可执行摘要**）。
- 本侧实现状态：A 段已合入 main；**B 段已落地**（`agent-facts`、collab 客户端、`channel-redrive`、`collab_send` 工具 + `collab/send` wire），真实对端联调通过。**S27 / S29 / S30 / S31 已全部落地**（见 §10）。

---

## 10. 实施结果（2026-10-04，对端回应）

三条阻塞项**全部落地**，另有一项由 S30 引出的必要伴生改动。全部改动经黑盒回归与消融验证。

### 10.1 S29 ✅

`collab/src/adapters/dsh.rs` 的 `is_definitely_absent()` 现在**只认 `unknown-agent`**：

```rust
matches!(self, Self::Rejected { code, .. } if code == "unknown-agent")
```

`PeerPresence::Absent` 的文档同步订正（`unknown-runtime` 不再算死亡）。**未新增码字**——§3.1 词表是封闭的，
`unknown-runtime` 的产出方式不变。

用例 `a_dsh_unknown_runtime_stays_uncertain_and_never_retires_the_route`：
`unknown-runtime` ⇒ `is_definitely_absent()` 假、`probe()` `Unknown`、`dsh_identity_presence()` `Unknown`
（**不是** `Missing`，故不授权退役）；反向对照 `unknown-agent` ⇒ 真、`Absent`、`Missing`。
消融：把谓词改回 `|| unknown-runtime` ⇒ 该用例在 presence 断言处变红。

### 10.2 S30 ✅ —— 但**真因与 §4.2 的描述不同**

§4.2 把原因写成"对端对已有 worker 保留了旧 transport"。实测不成立：`Event::Registered` 是**无条件**发射的，
reducer 也**无条件** `workers.insert` 覆盖，所以 transport 本身能更新。真因是 **`command_id` 去重**：

- 同 runtime key 重注册 ⇒ `reuse_existing` 为真 ⇒ `endpoint_generation` 不变
- ⇒ `command_id = register-<binding>-<generation>` 与首次注册**完全相同**
- ⇒ `commit_command_locked` 命中已提交回执，直接 `replayed:true` 返回
- ⇒ **一条事件都不写**，`worker.transport` 静默保留旧值

而 `same_runtime_key` 只由 binding 的 `(session_id, native_thread_id, tmux_endpoint)` 构成，
dsh 绑定的 `tmux_endpoint` 恒为 `None` ⇒ dsh 的 key 退化成 `(session, agent)`，
**网关 control socket 与 runtime id 完全不在判定内**。这是"重启后能发不能收"的唯一真源。

修复（`part_06.rs`）：dsh 的 runtime key 追加**地址**判定——旧 transport 的 `endpoint` 与 `namespace`
都必须等于本次候选。地址变了 ⇒ key 为假 ⇒ 走既有 rebind 分支 ⇒ `generation + 1` ⇒ 新 `command_id`
⇒ 新 `Registered` 事件带新 transport。非 dsh 恒真，tmux / appserver 行为不变。

**伴生必改项**：S30 落地后，"dsh 地址变更"第一次成为 rebind，于是 master 栅栏第一次对 dsh **可达**；
又因 S29 把 `unknown-runtime` 从 `Absent` 改成 `Unknown`，`live_master_id` 对 `Unknown` 返回 `Err`
⇒ dsh master 的自恢复会被 `MASTER_RECOVERY_BLOCKED_UNKNOWN` 拒绝。故补 `same_dsh_agent_recovery`：
同 token + 双方均 dsh + 同 agent id。**该判定必须不含地址**——地址正是变化量；与 tmux 的
`same_pane_tmux_recovery`（同 pane）对称。

> **关于 §4 的 S23**：原始论证（"dsh 重连必被栅栏拒绝"）在**改动前不成立**——`same_runtime_key` 恒真会短路栅栏，
> 反证用例 `a_dsh_master_reconnect_is_not_fenced_as_a_foreign_promotion` 已钉住该行为。
> 但 S30 落地后栅栏对 dsh 变为可达，因此**同主恢复判定确实需要**，只是触发条件是"地址变更"而非"任何重连"。

用例 `a_dsh_peer_returning_on_a_new_gateway_address_replaces_its_transport` 覆盖 §4.2 的三步验收：
(1) 首次注册后 journal 的 `Registered.transport.namespace == rt-1`；
(2) 换 socket + runtime 重注册 ⇒ `ok:true`，journal **新增**一条 `Registered`、`namespace == rt-2`、`endpoint` 为新 socket；
(3) 发消息 ⇒ 新 socket 收到 `enqueue`（`runtimeId == rt-2`），旧 socket **零** `enqueue`。
消融：去掉地址门 ⇒ 在"`Registered` 事件数 = 2"处变红（实测 `left: 1 / right: 2`）。

### 10.3 S31 ✅ —— 且**保持 AppServer 的对外 wire 值不变**

`part_05.rs:789` 现在把**真实 mailbox id** `first.1` 传给 `deliver` 的 `message_id`。

一个实现细节需要你知道：`deliver` 的第 4 参同时喂给三条 sink。AppServer 把它当 `clientUserMessageId`
直通进请求，而该命名空间与用户自己的消息**共用**，`collab-notification-` 前缀正是用来区分的
（本侧 live-closure 读取端也用 `strip_prefix` 还原 mailbox id）。因此前缀没有被删掉，
而是**移到 AppServer sink 内部**（新增 `appserver_client_message_id`）：

| 传输 | sink 收到的第 4 参 | 对外形态 |
|---|---|---|
| **dsh** | 裸 mailbox id | `messageId = collab:<digest>:<mailbox id>` ✅ |
| **AppServer** | 裸 mailbox id | `clientUserMessageId = collab-notification-<mailbox id>`（**逐字节不变**） |
| **tmux** | 裸 mailbox id | buffer 名由该值哈希而来（纯内部标签） |

⇒ 你的 `messageId` 形状修正了，而 AppServer 的 wire 值不变，无副作用。

用例 `a_dsh_wake_names_the_real_mailbox_id_so_a_peer_ack_can_match_it`：经真实 `send` 入口取 `data.msg_id`，
断言 `enqueue.params.messageId` **不含** `collab-notification-`、且形如 `collab:<16 位 hex>:<mailbox id>`。
同一改动也被 AppServer 侧既有用例观测到：`live_closure_daemon_send_uses_daemon_identity_without_explicit_source_thread`
的 sink 断言由 `collab-notification-{id}` 改为 `{id}`。
消融：恢复合成 id ⇒ 两条用例同时变红（`left: collab-notification-m1791177548416-4 / right: m1791177548416-4`）。

### 10.4 证据与基线

- 候选：分支 `codex/dsh-channel-s29-31`，base `d86807a`。
- 全量 `collab` 套件（真实 tmux 会话内串行 `--test-threads=1`）：**931 passed / 0 failed / 1 ignored**，exit 0。
  基线 928 ⇒ 本次新增 3 个黑盒回归用例。
- `cargo check --all-targets --offline` 退出码 0（仅剩仓库存量 warning）。
- 候选 release 构建：`./scripts/build-collab.sh --offline` 退出码 0，`collab_build_version=0.2.0213`。
- 消融：回退三处修复后 4 条用例全部变红且原因正确；回退后文件 sha256 与基线**逐一 OK**。
- **行号提醒**：§4 的行号测于 `9a9d674`。本轮按当时 HEAD `d86807a` 复核过：谓词与行为结论不变，
  个别行号有位移（例如 `dsh.rs` 的 `is_definitely_absent` 现位于 `:73`，`part_05.rs` 的投递点仍在 `:789` 一带）。
