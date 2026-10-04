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

## 7. 尚未实现、对端现在调不通（**预期，不是 bug**）

- 本侧控制面**目前没有 `agent-facts` op**。今天的 op 表只有：
  `runtimes` / `queue` / `enqueue` / `agents` / `agent-get` / `interrupt` / `hold-ack` / `release-ack` / `shutdown`。
  ⇒ 对端 `dsh.rs:232 facts()` 现在调用会收到 **`unknown-op`**。
- 也还没有 collab 客户端（`Req::Register` / `Ack` / `Send` / `Context` / `MsgStatus`）与 `channel-redrive`。
- 这些属 **B 段**，**等 S27 落地后再开工**。

**对端调用 `agent-facts` 的现状形状**（供双方对齐，B 段本侧会照此实现）：

```jsonc
// 请求（NDJSON，一行）
{"method":"agent-facts","params":{"nonce":"<challenge>","runtimeId":"<rt>","agentId":"<session>"}}

// 期望的 result（nonce 必须回显；AgentFacts 的五个字段都必需且均为字符串）
{"ok":true,"result":{"nonce":"<回显>","runtimeId":"…","agentId":"…","sessionId":"…","cwd":"…","status":"running|inactive"}}
```

- `nonce` **必须原样回显**（对端在 `dsh.rs:244-252` 校验，不回显 ⇒ `challenge-mismatch`）。
- **缺席必须走错误码**：`unknown-runtime` / `unknown-agent`（对端据此判 `Absent`），**不得**用 `status` 表达缺席。
- **无法判定**（如持久化能力缺席）走 `persistence-unavailable` ⇒ 对端判 `Unknown`，**绝不退休**。
- `status` 取值域是 **`running` | `inactive`**，由 `AgentView.live` 推导（见 §5 的 N25）。

---

## 8. 请对端做的事（checklist）

- [ ] **S27**：`dsh.rs` 的 `notify` 增 `messageId` 形参 + 写进 `enqueue` 请求体；`part_01.rs:196` 把已在作用域的 `message_id` 传下去；派生规则按 §3.2(c)；补 §3.4 的断言。
- [ ] **S22**：`part_02.rs:109` 的兜底 runtime 标签不要再产出 `runtime-dsh-appserver`。
- [ ] **S23**：dsh 的 master 恢复路径不要被 `same_pane_tmux_recovery` 恒判为阻塞（注意不要误改 `reissued_master_grant`）。
- [ ] **S25**：`part_10.rs:768/772/686` 的 rebind 文案不要对 dsh 指向 tmux。
- [ ] **复核 §5**：确认 S1–S3 / S5 / S18–S21 / S24 / S26 的"已落地"结论，并**保持 N25 的字面量 `"running"` 契约不变**。

---

## 9. 引用基线

- 本文件所有 appsdk 行号：**`9a9d674`** 实测（该 SHA 相对审计起点 `7a23f53` 只动了 `rust/`、`contracts/`、`docs/` 与证据，**`collab/` 未变**）。
  > 实施前请按**当时 HEAD** 重核一次行号——本文件的判定依据是**谓词与行为**，行号只是定位手段。
- 本侧契约：`COLLAB-CHANNEL-INTERFACE.md` **v10.1.9**（含 §9 的 S1–S27 完整审计账；本文件是它的**可执行摘要**）。
- 本侧实现状态：`docs/notes/a-stage-scope-cwd-run-notes.md`（A 段）；B 段未开工。
