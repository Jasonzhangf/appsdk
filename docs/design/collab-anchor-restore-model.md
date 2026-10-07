# Collab 身份恢复：锚点阶梯（Anchor Ladder）

**状态**：权威模型。与 `collab-identity-minimal-interaction.md` 配套；身份恢复的语义以本文件为准。
**唯一恢复代码路径**：`identity_resolver.rs::resolve_for_daemon_with_route_at`。

---

## 1. 原则：锚点是 adapter 设计出来的

锚点不是猜的。每个 transport 的锚点由该 transport 的 adapter 在适配时**设计并选定**，
来源是明确的环境变量或协议字段。锚点选定后写入持久化 identity，后续调用据此直恢，
不再重新猜测。

| transport | 锚点值 | anchor kind | adapter 设计来源 | 持久化位置 |
|---|---|---|---|---|
| tmux | `tmux_session_id` + `pane_id`（一对） | `tmux_session_id` | `candidate_from_env` 读 `TMUX_PANE` → tmux 查询 `#{session_id}` `#{pane_id}` | `transport.tmux_endpoint` |
| appserver | `session_id` | `codex_session_id` | env `CODEX_SESSION_ID` | `runtime.session_id` |
| appserver | `native_thread_id` | `codex_thread_id` | env `CODEX_THREAD_ID` | `runtime.native_thread_id` |
| dsh | `session_id` | `dsh_session_id` | env `DSH_SESSION_ID` | `runtime.session_id` |

锚点值必须是**非空的稳定标识**，且可被 `validate_id` 接受（持久化到身份目录名）。

**tmux 只用一对**。`session_id` 单独匹配会让同一个 tmux session 的两个 pane 命中同一条
身份，从而在跨 pane 场景下产生歧义或直接吞掉另一个 pane 的身份。因此 tmux 锚点是
「同 socket、同 session、同 pane」的组合，由 `same_owned_pane` 判定：session id 是主
成分，pane id 是同一 session 内的判别符。

## 2. 恢复阶梯

```
observed anchors
  │
  ├─ 1. 锚点直恢    锚点值 == 某条持久化 identity 的锚点值
  │         tmux owned pane 配对 / codex session|thread 命中 / dsh session 命中
  │         → 恢复该 identity，token、runtime、binding 全部沿用
  │         → 通过既有 Register 事务以当前提供的证据重新提交
  │
  ├─ 2. 无匹配起草  无任何持久化锚点命中
  │         tmux      → worker_id = codex-<pane_id>
  │         appserver → worker_id = codex-thread-<thread_hex>
  │         dsh       → worker_id = dsh-thread-<session_hex>
  │         → 已有草稿沿用其 token；不存在则 mint 新身份
  │
  └─ 3. 失败关闭    无法唯一归属
            同一锚点匹配多条 → IDENTITY_RESTORE_AMBIGUOUS
            唯一匹配到别的 project → IDENTITY_RESTORE_CROSS_PROJECT
            完全没有锚点 → COLLAB_IDENTITY_ANCHOR_MISSING
            均不回退猜测，均不产生身份
```

阶梯第 3 级不是错误回退。歧义和跨项目是**归属无法唯一确定**，只有用户能裁决。

### 2.1 锚点直恢

**唯一条件**：观测到的锚点值等于某条已持久化 identity 的持久化锚点值。

- 不需要任何额外理由（runtime 状态、liveness probe、`endpoint_generation` 一概不需要）。
- 不需要额外证据。
- 恢复后，该 identity 的其他字段由当前调用提供的证据更新，通过既有 Register 事务提交。
- 同一条锚点匹配多条 → `IDENTITY_RESTORE_AMBIGUOUS`。
- 唯一匹配到其他项目 → `IDENTITY_RESTORE_CROSS_PROJECT`。

### 2.2 无匹配起草

无任何持久化锚点命中时，按当前观察到的锚点起草新身份。worker id 由锚点值确定性生成，
因此同一个无历史锚点重复调用得到同一条身份；已有草稿沿用其 token，不重新签发。

### 2.3 用户裁决

只有 master 归属需要用户裁决，它独立于上面的身份解析。`collab master promote --approval
<text>` 携带显式授权，以调用者自己的 worker id 授予 master，替换已记录的 incumbent。
该命令不接受 liveness 咨询：显式授权本身就是全部依据，因此 tmux pane 是否可达、
AppServer oracle 是否在线都不影响裁决。裁决持久化后，下一次调用走阶梯第 1 级直恢。

普通 peer 注册不需要裁决，它就是普通 Register 的结果。

---

## 3. 不变式

1. **恢复结果唯一**：同一锚点匹配多条 → `IDENTITY_RESTORE_AMBIGUOUS`，不猜测。
2. **项目隔离**：跨项目锚点匹配 → `IDENTITY_RESTORE_CROSS_PROJECT`，失败关闭。
3. **裁决即终态**：master 裁决持久化后，下次调用直接恢复，无需再次裁决。
4. **锚点不可观测不阻断**：某个 transport 的锚点不可观测（env 缺失）不阻断其他 transport 的恢复。
5. **无锚点不得有身份**：三种 transport 的锚点都缺失 → `COLLAB_IDENTITY_ANCHOR_MISSING`。

---

## 4. CLI 接口

| 命令 | 作用 |
|---|---|
| `collab context` | 唯一入口。CLI 观察锚点 → daemon 走阶梯 → 返回 snapshot |
| `collab context --provide '<JSON>'` | 一次补齐缺失事实；只接受 `session_id`、`thread_id`、`endpoint`、`namespace` |
| `collab master promote --approval <text>` | 携带用户 master 裁决 |

`DSH_SESSION_ID` 由 CLI 自动观察，**不在 `--provide` 里**。锚点是观测事实，不是 agent 提供的
参数。没有 `collab init`，没有 `--restore-as`。

## 5. `required_fields` 语义

`required_fields` 只对 AppServer 路径返回非空。tmux 候选与 dsh 锚点各自自带完整锚点，
返回空列表。一个只带着 `DSH_SESSION_ID` 的调用者不会被要求补 AppServer 的四件套。

**DSH 身份创建仍由 gateway 拥有，CLI 不能绕过它。** `DSH_SESSION_ID` 只解决"我是谁"
（锚点观测与 `required_fields`），不解决"我如何注册"。`DshCandidate` 需要 gateway 的
`endpoint`、`runtime_id`、`agent_id` 三件套，CLI 只能观测 session id，没有 gateway 就无法
构造 candidate。新 dsh 身份在 gateway 缺席时失败于 `TRANSPORT_NONE`——这是 gateway 的
失败，不是身份缺失。已持久化的 dsh 身份由锚点直恢（阶梯第 1 级）判定身份归属；presence
与注册仍需 gateway 在线。

## 6. 与既有设计的关系

- `collab-identity-minimal-interaction.md` 保留为交互契约（一条命令、无参数负担）。
  本文件是其身份恢复部分的权威补充。
- `docs/dagpipe/collab-context.graph.json` v0.7.0 的 `identity_gate` 节点是本文件的图化。
- DSH 通道契约（gateway `agent-facts` challenge）见
  `dsh-gateway/b-stage-collab-client/docs/COLLAB-DSH-CHANNEL-DESIGN.md`。本文件只声明
  dsh 的锚点是 `session_id`。
