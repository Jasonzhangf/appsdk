# Collab 身份恢复：锚点阶梯（Anchor Ladder）

**状态**：权威锚点模型。与 `collab-identity-context-recovery-contract-20261008.md` 配套；锚点阶梯以本文件为准。
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

自动锚点阶梯先处理身份归属。没有唯一可采纳锚点时，只有用户批准裁决可以覆盖
身份选择，且该裁决必须精确到 project/app scope、target identity、action、
规范化 intent digest，以及被替换的 incumbent binding/generation。
普通 peer 注册不需要批准，它就是普通 Register 的结果；唯一锚点直恢也不需要
额外批准。

用户批准恢复和 master grant 是不同的控制决定，属于不同的 owner：

- 批准身份恢复只授权对该精确目标和 scope 的身份/binding 裁决。它不是 token、
  endpoint 或 scope 的所有权证明，daemon 仍必须验证当前端点。批准提交必须在
  首个身份副作用前持久化外层 operation id；批准后 incumbent 变化时返回
  `APPROVAL_STALE_CONFLICT`，不得把同一次批准套用到新 binding。
- 旧 credential 或旧 binding 失效不构成自动覆盖授权。只有显式批准恢复 invocation
  可以进入 daemon 的 approved replacement transaction；普通 Register 和普通
  authenticated command 继续保留 token、binding、generation 与 scope 拒绝，
  `TOKEN_MISMATCH` 只描述这些普通 admission 路径，不描述已批准的恢复事务。
- 恢复事务的真实 binding id 由 Register owner 提交，内部 admission proof 不是
  token，也不会伪造 `actor_binding_id`。
- `collab master promote --approval <text>` 只变更 master authority，不隐式改变
  身份，也不隐式 clear 或 transfer 其他授权；authority 合同见
  `collab-master-authority-contract-20261007.md`。
- 恢复原 master 身份不自动 grant、clear 或 replace。若同一次用户意图同时包含
  身份恢复和 master 授权替换，设计合同要求两个 owner 的步骤分别提交并在外层
  operation 中并列为两个 phase，而不是互相冒充。

批准裁决持久化后，下一次调用走阶梯第 1 级直恢。liveness、runtime state、
recency 和 pane 名称都不是批准依据，也不影响裁决记录是否有效。

D2-B 为批准恢复冻结完整的外层 operation、phase/replay/query 和拒绝终点，
见 `collab-identity-context-recovery-contract-20261008.md` 的
“3. One invocation, one discriminated result”及
“5. Approved stale-credential recovery seam”。本节的批准语义是该合同的
恢复部分权威正文；两份文档冲突时以该合同为准。

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
| `collab context --op <operation-id>` | 唯一入口的自动路径。CLI 观察锚点，daemon 走阶梯并返回判别结果 |
| `collab context --op <operation-id> --provide '<JSON>'` | 一次补齐缺失事实；只接受 `session_id`、`thread_id`、`endpoint`、`namespace` |
| `collab context --op <operation-id> --approve-identity '<JSON>'` | 显式批准身份恢复或 binding replacement；JSON 含 target、scope、action、incumbent binding/generation 和 intent digest |
| `collab context --op <operation-id> --approve-grant '<JSON>'` | 显式批准 master grant replacement；与 identity approval 是不同 owner phase，不能互相替代 |
| `collab context --op <retained-operation-id> --query` | 纯读取外层 operation 的 durable phase projection；旧 credential 失效时仍可用保留的 key 和 proof 查询，不允许 repair |
| `collab master promote --approval <text>` | 携带用户 master authority 裁决，不是身份恢复 |

`DSH_SESSION_ID` 由 CLI 自动观察，**不在 `--provide` 里**。锚点是观测事实，不是 agent 提供的
参数。D2-B 不新增第二个用户选择命令；批准恢复通过同一 `collab context` 入口的 typed
invocation 提交，并由 daemon 身份门持有。CLI 仍不提供 `--restore-as` 这种直接指定并接管
身份的旁路。`collab context --query` 不返回 token，也不修复 credential、binding、route、
grant 或 lease；若需要 repair，必须提交带新 operation intent 的显式 `collab context`
调用，不能把 query 当作写入口。`collab init` 不是第二条身份路径：它与 `collab context` 共用同一 daemon 身份门
（`identity_gate`），只是为既有 AppSDK init 消费者输出既有响应形状；歧义与跨项目在两条入口
上都保持 fail-closed，见 `docs/design/collab-identity-shortest-path.md`。

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

- `collab-identity-context-recovery-contract-20261008.md` 是 D2-B 的规范契约。
  `collab-identity-minimal-interaction.md` 保留为兼容镜像，供已有设计引用。
- `docs/dagpipe/collab-context.graph.json` v0.7.0 的 `identity_gate` 节点是本文件的图化。
- DSH 通道契约（gateway `agent-facts` challenge）见
  `dsh-gateway/b-stage-collab-client/docs/COLLAB-DSH-CHANNEL-DESIGN.md`。本文件只声明
  dsh 的锚点是 `session_id`。
