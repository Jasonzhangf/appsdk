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
| tmux | `socket_path` + `pane_id` | `tmux_pane` | `anchor_from_env` 读 `TMUX` / `TMUX_PANE`；live endpoint enrichment 独立 | `transport.tmux_endpoint` |
| appserver | `session_id` | `codex_session_id` | env `CODEX_SESSION_ID` | `runtime.session_id` |
| appserver | `native_thread_id` | `codex_thread_id` | env `CODEX_THREAD_ID` | `runtime.native_thread_id` |
| dsh | `session_id` | `dsh_session_id` | env `DSH_SESSION_ID` | `runtime.session_id` |

锚点值必须是**非空的稳定标识**，且可被 `validate_id` 接受（持久化到身份目录名）。

**tmux 只用一对**。`session_id` 单独匹配会让同一个 tmux session 的两个 pane 命中同一条
身份，从而在跨 pane 场景下产生歧义或直接吞掉另一个 pane 的身份。因此 tmux 锚点是
「同 socket、同 pane」的组合，由 `same_owned_pane` 和 `same_pane_route` 判定。
session、server/pane PID、native IDs、runtime、activity 与 capture 都不参与身份选择。

## 2. 恢复阶梯

```
observed anchors
  │
  ├─ 1. 锚点直恢    锚点值 == 某条持久化 identity 的锚点值
  │         tmux owned pane 配对 / codex session|thread 命中 / dsh session 命中
  │         → 恢复该 identity，token、runtime、binding 全部沿用
  │         → 读取 durable selected binding 与 authority，直接投影；不重选、不 Register、不回环
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
- 恢复后沿用 durable 身份、角色、anchor 与 selected binding。live display/capture 失败不阻断稳定 socket+pane 请求；daemon 从 durable owner 补全已保存字段，不填造 PID/native IDs。
- 普通恢复不重新准入 adaptor、不 Register、不回环。合法相邻 generation 的 staged publication 收尾继续走既有 typed Register owner，并保留 token、scope、generation 与 publication guards。
- 同一条锚点匹配多条 → `IDENTITY_RESTORE_AMBIGUOUS`。
- 唯一匹配到其他项目 → `IDENTITY_RESTORE_CROSS_PROJECT`。

### 2.2 无匹配起草

无任何持久化锚点命中时，按当前观察到的锚点起草新身份。worker id 由锚点值确定性生成，
因此同一个无历史锚点重复调用得到同一条身份；已有草稿沿用其 token，不重新签发。

### 2.3 用户裁决

自动锚点阶梯先处理身份归属。没有唯一可采纳锚点时，只有用户批准裁决可以覆盖
身份选择。裁决命名 target identity、action 和用户覆盖意图；scope 从当前 invocation
确定。daemon 在身份锁内读取当前冲突集合和执行 fence，不要求用户查询旧 fence。
普通 peer 注册不需要批准，它就是普通 Register 的结果；唯一锚点直恢也不需要
额外批准。

用户批准恢复和 master grant 是不同的控制决定，属于不同的 owner：

- 批准身份恢复只授权对该精确目标和 scope 的身份/binding 裁决。它不是 token、
  endpoint 或 scope 的所有权证明。daemon 验证持久归属、地址结构和实际提交效果，
  不重复 live probe。批准提交在首个身份副作用前持久化外层 operation id。
  当前 incumbent 漂移由同一次 invocation 在锁内确定，不要求第二次批准。
- 唯一同 pane 归属自动同步旧 credential、binding 与 daemon 漂移。冲突覆盖走
  显式批准的既有 replacement transaction；普通 Register 和普通
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
| `collab context --op <operation-id> --approve-identity '<JSON>'` | 一次显式批准身份恢复或覆盖；JSON 命名 target、action 和用户意图，daemon 读取当前 incumbent |
| `collab context --op <operation-id> --approve-grant '<JSON>'` | 既有 grant replacement；也可承载明确 `restore_master` / `restore_peer` 的单次用户意图，实际角色效果由 authority owner 提交 |
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

- 当前注册与显式手动通信恢复的区分见
  [规范合同第 2 节](collab-identity-context-recovery-contract-20261008.md#2-terms-and-exact-owner-map)。
  当前适用需求以项目 `.appsdk/requirements.json` 的有效记录及用户后续明确指令为准。
  `collab-identity-minimal-interaction.md` 仅保留为兼容入口，引用规范正文。
- `docs/dagpipe/collab-context.graph.json` v0.7.0 的 `identity_gate` 节点是本文件的图化。
- DSH 通道契约（gateway `agent-facts` challenge）见
  `dsh-gateway/b-stage-collab-client/docs/COLLAB-DSH-CHANNEL-DESIGN.md`。本文件只声明
  dsh 的锚点是 `session_id`。
