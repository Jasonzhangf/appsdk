# Collab 身份认定与恢复设计

**状态**：设计稿（本仓库自有设计产物）；落盘后作为实现与 review 的准入依据
**基线**：`c5007adc427de53c8aa8c55097daa56e2f506466`（与 origin/main 同步）
**行号约定**：除注明外均按上述基线，仅在该提交上定位
**关联产物**：`collab/docs/design/identity-route-ledger-maintenance-dag.md`（账本维护专项）、`docs/dagpipe/manifest.json`（图产物注册表）
**范围**：身份分层、恢复规则、规则清单、SESE/DAG 设计、实施拆解、验收矩阵
**非范围**：DSH 协议实现（留白）、AppSDK `rust/` 三 store 的重构（只定义关联字段与验收终点）

---

## 1. 需求与硬约束

本设计的验收以以下需求为准，逐条编号，后续章节必须能追溯到它们。

| 编号 | 需求（来源：用户明确要求） | 本设计的落点 |
|---|---|---|
| R1 | **任何情况下都可以恢复 collab 身份** | §3 恢复规则；§5 F1/F2 两条恢复 DAG；§3.3 每个失败终态必须可修复 |
| R2 | live 不冲突一定要能恢复身份**和通讯** | §3.4 订阅归属；§3.2 判定顺序 R-4/R-8 |
| R3 | 冲突由用户裁决，**可以顶替冲突身份** | §2.5 + §4.2 手动裁决通道（M1–M7） |
| R4 | 有 tmux 永远有 pane，不可能没有身份；**没有 tmux 要报错** | §3.4 零锚点分支：有 tmux 走 pane 锚点；无 tmux 显式报错，不静默 mint |
| R5 | 后台自动管理、自动恢复、同步；需要时让 peer 提供更新信息 | §5.2 F1 无交互入口；§4.2 M5 手动通道仅限交互式 CLI，不参与后台路径 |
| R6 | 不静默失败、不未经授权破坏、无证据不宣称完成 | §3.3 可修复终态；§4.4 禁止项 |

**硬约束（来自项目治理）**：控制面与业务 payload 物理隔离；错误显式暴露；禁止无条件 fallback 与双路径补偿；禁止 `pkill`/`killall`/`kill $(...)`；项目代码修改必须在独立 worktree 完成。

---

## 2. 身份划分

五层，互不替代。每层给出：定义、唯一键、创建入口、基数、判死条件、与现状映射。

### 2.1 Principal（主体）

| 项 | 定义 |
|---|---|
| 语义 | 稳定、不可变的身份主体。"谁" |
| 唯一键 | `principal_id`（host 全局命名空间唯一） |
| 创建入口 | **唯一入口** `collab identity create`。禁止从环境变量、pane 名、cwd、`COLLAB_WORKER` 隐式创建 |
| 基数 | 1 Principal : N Credential : N RuntimeInstance |
| 判死条件 | **永不自动删除**。注销只写 tombstone；只有显式 GC 授权后才物理移除 |

**凭据归属（补齐原设计缺口）**：token 属于 **Principal**，不属于通道或 binding。一个 Principal 可以有多个 credential（`credential_id`，含 `created_ms`/`revoked_ms`），用于轮换与多设备。**撤销 credential 不删除 Principal**，只令该凭据签发的证据失效。

**与现状映射**：`identities/<worker_id>/identity.json` 的 `worker_id`/`token` → `principal_id`/`credential`。迁移期 `principal_id == worker_id`（同值），旧档案标记 `legacy-unverified`（动作白名单见 §6.3）。`worker_id` 降级为 Principal 的兼容别名。

### 2.2 RuntimeInstance（运行实例）

| 项 | 定义 |
|---|---|
| 语义 | 一次具体的运行实例（持有通道端点的那个进程的一次生命周期）。"哪个实例" |
| 唯一键 | `runtime_instance_id`，不可变 |
| 创建入口 | daemon 在首次接受该实例的证据时 mint；输入 `(boot_id, process_start_id, endpoint_identity)` |
| 基数 | N RuntimeInstance : 1 Principal |
| 判死条件 | 仅当 OS 证明该 `(boot_id, process_start_id)` 不存在（或 `boot_id` 变化）时判死。**probe 失败/超时 → Unknown，不判死** |

**进程边界（补齐原设计缺口）**：`RuntimeInstance` 指**持有通道端点的进程**——tmux peer 是 pane 内的进程（`pane_pid` 及其后代），App Server peer 是 App Server 进程（socket 的 peer）。**daemon 不是 peer 的运行实例**：daemon 重启不改变任何 `RuntimeInstance`，只把存量 lease 置 `Unknown` 并要求重新 challenge。

同一 `(boot_id, process_start_id)` 复用同一 `runtime_instance_id`；进程重启 → 新实例，旧实例留 tombstone。

### 2.3 ChannelBinding（通道绑定）

| 项 | 定义 |
|---|---|
| 语义 | Principal + RuntimeInstance 在某项目 scope 下使用某通道端点的短期绑定 |
| 唯一键 | `binding_id`；同键下 `binding_generation` 单调递增 |
| 创建入口 | registration CAS（`expected_revision` 匹配才提交） |
| 基数 | **(principal_id, runtime_instance_id, project_scope) 最多 1 个 active binding** |
| 判死条件 | 显式 revoke，或 generation 前进取代（旧 generation 保留只读） |

**基数与优先级（补齐原设计缺口）**：现状允许 client 同时提交 AppServer 与 tmux candidate（daemon 优先 AppServer、tmux 留作 pane recovery anchor，见 `identity-route-ledger-maintenance-dag.md`）。目标模型把它规范化为**一个 binding 带一个 primary endpoint + 零或一个 recovery anchor**：

- primary endpoint 优先取验证通过的 AppServer endpoint；
- 仅当 AppServer endpoint 缺席或验证失败时，tmux pane 才成为 primary；
- tmux pane 作 recovery anchor 时**不单独构成 binding**，只作 R-4/R-5 的锚点证据与恢复提示。

### 2.4 Presence 与 Grant

| 项 | 定义 |
|---|---|
| PresenceLease | `{binding_id, generation, state, observed_at_ms, ttl_ms}`，state ∈ Present/Cold/Missing/Unknown |
| 写入者 | **只有 daemon 在 challenge 通过时写 `Present`**；probe 结果只能把 lease 降级或置 `Unknown`，**不能升级为 Present**（对 F-03 的规则化） |
| Grant | 授权授予（master/role/route 写权限），按 (principal, scope, role) 维护，独立于 presence |

失效条件互不蕴含：binding 存在不证明 lease 有效；lease 到期不删除主体；grant 失效不代表进程死亡；任务角色是授予/派生视图，不反写身份分类。

### 2.5 ManualAdjudication（手动裁决）

人的显式决定层，与自动准入严格分离。它不是"更宽松的准入"，而是**另一条授权来源**：自动准入的授权来自证据 quorum，手动裁决的授权来自操作者的声明。约束见 §4.2。

| 项 | 定义 |
|---|---|
| 唯一键 | `receipt_id` |
| 创建入口 | 仅交互式 CLI：`collab identity adjudicate --worker <id> --scope <root>`（T1；T0 期入口为 `collab context --worker <id>`，见 §3.2 T0 实现状态） |
| 产出 | `AdjudicationReceipt{receipt_id, operator, at_ms, principal_id, source_scope, target_scope, generation_before/after, inherited_subscriptions, ttl_ms, revoked_ms}` |
| 基数 | N receipt : 1 principal（每次裁决一条，可审计） |
| 判死条件 | TTL 到期或显式撤销（`collab identity revoke <receipt-id>`） |

### 2.6 分层基数总表

| 层 | 唯一键 | 基数 | 创建入口 | 判死条件 |
|---|---|---|---|---|
| Principal | `principal_id` | 1:N credential, 1:N runtime | `collab identity create` | 永不自动删除 |
| Credential | `credential_id` | N:1 principal | `collab identity credential add` | `revoked_ms` 非空 |
| RuntimeInstance | `runtime_instance_id` | N:1 principal | daemon mint | OS 证明进程不存在 |
| ChannelBinding | `binding_id` | 1 active per (runtime, scope) | registration CAS | revoke / generation 前进 |
| PresenceLease | `binding_id`+`generation` | 1 per binding | challenge 通过 | TTL 到期 → Unknown |
| Grant | (principal, scope, role) | N | approval grant | revoke |
| AdjudicationReceipt | `receipt_id` | N:1 principal | 手动裁决 | TTL 到期 / 撤销 |

---

## 3. 恢复规则

### 3.1 恢复入口

**唯一入口**：`collab context`（T0 只有 `--worker`；`--scope` 与独立裁决入口 `collab identity adjudicate` 属 T1，见 §3.2 T0 实现状态）。其他路径（MCP、daemon 内部、后台同步）不得成为独立入口，只能触发同一入口的既有分支。

后台自动恢复（R5）走 F1（§5.2），不需要用户触发；需要额外信息时由 peer 侧在下一次 `collab context` 提供。

### 3.2 恢复判定顺序表

**这是本设计新增的核心规则**：多个条件同时成立时，按编号从高到低取第一个命中，结果唯一。不得由实现者自行决定顺序，也不得被配置重排。

| 编号 | 条件 | 结果 | 终态 | 可修复 |
|---|---|---|---|---|
| R-1 | credential 已撤销 / security deny | 拒绝 | `IDENTITY_REVOKED` | 需新建 principal |
| R-2 | nonce/replay 失败，或同 binding 多 principal，或不同 subject claims | Conflict | `IDENTITY_CONFLICT` | 需裁决 |
| R-3 | 记录 `project_scope` ≠ 当前 scope，且无显式裁决 | **目标**：同锚点全部 duplicate 可证 Dead → retire + mint，否则 fail-closed。**T0 现状**：自动路径不跨 scope，一律 fail-closed 并指向 `--worker`（见 §6.1 T0-1） | `IDENTITY_CROSS_PROJECT` | 需裁决；dead-only 自动 retire 属 T1 |
| R-4 | 存在 live owner（他 scope 或同 scope）claim 同一锚点 | fail-closed | `IDENTITY_AMBIGUOUS` | 需裁决 |
| R-5 | 无当前锚点（TMUX_PANE / CODEX_SESSION_ID / CODEX_THREAD_ID 全空）且未命名 | 未认证 | `IDENTITY_ANCHOR_MISSING` | 提供锚点或裁决 |
| R-6 | 显式裁决：`--worker <id>`（T0）/ `--worker <id> --scope <root>`（T1，M2） | 手动裁决 | `ADJUDICATED` | — |
| R-7 | 证据域 < 2，或证据过期 / Unknown | Unproven | `IDENTITY_UNPROVEN` | 重新 challenge |
| R-8 | 全部必选证据通过且 quorum 达标 | Accepted | `ACCEPTED` | — |

**R-6 的边界（关键约束）**：手动裁决只能越过 **R-3 / R-4 / R-5**（锚点与 scope 类）。**不能越过 R-1 / R-2**（凭据撤销与声明冲突）——被撤销的凭据不能靠人声明复活，声明冲突也不能靠人声明掩盖。这一条修正了"裁决可越过一切"的宽松表述。

**T0 实现状态（本设计落盘时点的真实契约）**：F2 的独立入口 `collab identity adjudicate`、`--scope` 声明（M2）与 durable receipt（M3）尚未实现；T0 期的手动裁决借用既有 `collab context --worker <id>`，其 scope 由 cwd/route 派生。因此 T0 的对外契约是：

- **F1（无 `--worker`）不跨 scope**：跨 scope 记录一律 fail-closed 到 `IDENTITY_CROSS_PROJECT`，错误文本把用户指向 `--worker`。这与 `c5007ad` 的 ledger 契约一致（"the implicit path is unchanged: it never crosses scope"）。
- `Req::Register.retire_cross_project_anchor` **只在显式 `--worker` 时置位**；F1 路径不得置位，否则会跳过同 scope 的 live 冲突拒绝（`retire_cross_project_anchor_candidate` 对同 scope binding 是 no-op）。
- M2/M3 的强制、dead-only 自动 retire、以及退休的事务性（retire 与 binding 原子提交 + tombstone + 失败回滚）属 T1，缺口与证据见 §8.2。

### 3.3 失败终态（必须可修复）

每个失败分支必须落到一个终态，携 `reason` + `repair_required` + **可执行命令模板**。禁止只回错误码，禁止静默降级。

| 终态码 | 原因 | repair_required | 修复入口 |
|---|---|---|---|
| `IDENTITY_REVOKED` | 凭据已撤销 | 是（需新建） | `collab identity create` |
| `IDENTITY_CONFLICT` | 声明冲突 / 多 principal | 是 | 冲突清单 + `collab identity adjudicate` |
| `IDENTITY_CROSS_PROJECT` | 跨 scope 且无裁决 | 是 | T0：`collab context --worker <id>`（scope 由 cwd/route 派生）；T1：`collab identity adjudicate --worker <id> --scope <root>` |
| `IDENTITY_AMBIGUOUS` | live owner 占用锚点 | 是 | 同上（输出 owner 摘要） |
| `IDENTITY_ANCHOR_MISSING` | 无锚点且未命名 | 是 | 提供锚点，或显式裁决 |
| `IDENTITY_UNPROVEN` | 证据不足 / 过期 / Unknown | 是 | 重新 challenge（`collab context` 重跑） |
| `ADJUDICATED` | 手动裁决成功 | 否 | — |
| `ACCEPTED` | 自动准入成功 | 否 | — |

### 3.4 零锚点与订阅归属

**零锚点分支（R4 落地）**：

```text
无 TMUX_PANE / CODEX_SESSION_ID / CODEX_THREAD_ID
  ├─ 有 tmux（TMUX/TMUX_PANE 存在）→ 必然有 pane → 走 pane 锚点分支（R-3/R-4/R-5 正常判定）
  └─ 无 tmux → 显式报错 IDENTITY_ANCHOR_MISSING，不静默 mint、不猜测
```

**订阅与通讯归属（R2 落地）**：

- 现状：`NotificationSubscription.worker_id`（`collab/src/server/state.rs:214`）绑定的是名称键。
- 目标：订阅归 **Principal**（`principal_id`）。语义是"谁要收"，与运行实例、通道、binding 代际无关。
- **恢复同一 Principal ⇒ 订阅自动延续**，无需重建。这是"通讯可恢复"的实现基础。
- **顶替他人身份的副作用必须显式**（R3）：手动裁决领养另一个 Principal 时，会继承该 Principal 的全部 active 订阅与 direct-message lease。因此：
  - 裁决输出**必须列出将被继承的订阅清单**，并写入 receipt 的 `inherited_subscriptions`；
  - 提供 `--no-inherit-subscriptions`：选择不继承，则新 principal 的订阅集为空，需要重新订阅；
  - 不提供该选项时，默认继承但**必须打印清单**，不允许静默接管他人的通知链路。
- binding generation 前进**不影响**订阅；订阅只在显式 unsubscribe、Principal tombstone、或 lease 型订阅到期时终止。
- mailbox 只承载 payload；通知链路（订阅）与投递（consumption）是两个独立闭环，不得互相替代。

---

## 4. 规则清单（落盘规则）

以下规则是本设计的**强制约束**，实现与 review 都必须按其判定。每条给出违规后果。

### 4.1 不变量 I1–I8（重构不得回退）

| # | 不变量 | 现状证据 | 违规后果 |
|---|---|---|---|
| I1 | 无当前锚点且未命名 ⇒ 未认证（fail-closed） | `identity_for_scope_rebind_at` 无锚点门；live 验证 exit=1 报 `IDENTITY_REBIND_UNPROVEN` | 静默 mint 出无锚点身份 |
| I2 | live 冲突 fail-closed；只有 R-6 显式裁决可越过 | live 冲突判定先于 scope 过滤 | 隐藏他 scope 的 live owner |
| I3 | 只有同锚点全部 duplicate 可证 Dead 才允许归档 | 跨项目退休门槛；survivor 存在时 Dead 不取胜 | 误删存活 peer 的归属 |
| I4 | 显式 `--worker` 不做 liveness probe | 选中分支按路径直读 | 探测超时导致无法恢复 |
| I5 | 锚点按 principal 归并去重 | `anchor_groups` 归并 | 同一 peer 被当成多个候选 |
| I6 | 同 pane master 顶替以 host 级 pane 归属判定为准：pane 上存在不属于本 binding 自身 route 的 claim，即表示该 pane 归属他人；请求围栏仅在无人拥有该 pane 时报 `RECOVERY_RECONCILE_REQUIRED`，republisher 以 `RECOVERY_RECONCILE_SKIPPED_SUPERSEDED` 跳过而不驱逐 | `GlobalState::pane_claimant_other_than`（`collab/src/server/global_state_impl.rs:561`）与其决策包装 `ProjectRuntimeManager::pane_owner_other_than`（`collab/src/server/mod_parts/runtime_manager_setup.rs:451`） | 无 host 级 pane 归属判定就顶替 master 或围栏项目 route |
| I7 | probe 错误只在明确文本下判 Dead，其余 Unknown | `classify_probe_error`（仅 `no rollout` / `thread not found` / `tmux_pane_missing`） | 畸形响应被当成死亡证明 |
| I8 | pane 同一性比较必须含 `pane_pid`（5 字段） | `collab/src/adapters/tmux.rs:308-314` 的 `same_pane_route`（唯一实现；调用点唯一，见 T0-3） | pane id 复用被误认为同一 pane |

每条必须有对应回归测试；删除任一实现必须使对应测试变红。

### 4.2 手动裁决约束 M1–M7

| # | 规则 | 违规后果 |
|---|---|---|
| M1 | 授权来源是操作者的显式声明，**不是**证据 quorum；不得被记为 Accepted decision | 裁决被误当成 quorum 通过 |
| M2 | 必须同时给出 `--worker` 与 `--scope`；缺一即拒绝，不得从 cwd/环境变量/旧 route 推断 scope | 隐式接管 foreign scope |
| M3 | 必须产出 durable receipt（含继承订阅清单），且出现在 `collab context` 输出中 | 无法审计的静默接管 |
| M4 | receipt 携 TTL，可撤销；撤销回退身份指针并保留旧 binding 只读，**禁止 generation 回退** | 撤销复活旧 context |
| M5 | 仅交互式 CLI 入口。配置文件、`COLLAB_WORKER`、MCP、daemon 后台路径一律不得触发 | 非交互路径绕过准入 |
| M6 | 只能恢复身份与通道绑定，**不得**授予 master/role，不得改 policy epoch，不得写 grant | 借恢复提权 |
| M7 | 不得越过 R-1（凭据撤销）与 R-2（声明冲突） | 撤销失效、冲突被掩盖 |

### 4.3 证据域判据 D1–D5

| # | 规则 |
|---|---|
| D1 | 域按"谁签发 + 如何独立观测"划分：`credential_store`、`tmux_server_control`、`host_kernel`、`appserver_rpc`；`local_file` **不得作为证据**，只作 locator |
| D2 | 同一进程、同一环境导出的多字段（`TMUX`/`TMUX_PANE`/`CODEX_SESSION_ID`/`CODEX_THREAD_ID`）属于**同一域**，无论几个字段都只算一组 |
| D3 | 同一 RPC 响应的多字段（session/thread/cwd/status）属于**同一域** |
| D4 | `min_independent_groups >= 2` 由代码锁定；validator 必须拒绝任何把它降到 1 的 profile；`weight` 只用于可选评分，不能补必选项 |
| D5 | 环境变量、cwd、显示名、`worker_id` 一律进 locator/hints，不进 evidence |

### 4.4 禁止项

- 禁止把文件 mtime 作为授权依据（只作 locator 提示）。
- 禁止把环境变量、进程名、pane 名作为身份证据。
- 禁止用配置降低 hard floor，或把 Unknown 当作满足必选。
- 禁止静默降级：任何拒绝必须落到 §3.3 的终态码并给修复入口。
- 禁止把裁决记成 quorum 通过，禁止把裁决 receipt 当 grant 使用。
- 禁止在自动路径（F1）中回边修改 F2 的真源，反之亦然。

---

## 5. SESE / DAG 设计

### 5.1 功能切分（修正原设计的双源问题）

身份恢复是**两个独立功能**，各自 SESE（单源单汇、每节点单入口单出口），共享同一个 sink：

| 功能 | 源（唯一入口） | 汇（唯一出口） | 授权来源 |
|---|---|---|---|
| **F1 自动准入恢复** | `collab context`（无 `--worker`/`--scope`） | `ACCEPTED` 或 §3.3 失败终态 | 证据 quorum |
| **F2 手动裁决恢复** | `collab identity adjudicate`（显式 `--worker` + `--scope`）；T0 期由 `collab context --worker` 承担，见 §3.2 T0 实现状态 | `ADJUDICATED` receipt | 操作者声明 |

**共享 sink**：`identity/binding` 写入点。约束：
- sink 必须是幂等 CAS（`expected_revision` + `expected_generation`），两个功能都只能通过它写入；
- F2 不得向 F1 的真源回边：不写 decision journal、不改 policy epoch、不写 grant；
- F1 不得消费 F2 的 receipt 作为证据。

### 5.2 F1 自动准入恢复 DAG

| 节点 | owner | 输入 | 输出 | 说明 |
|---|---|---|---|---|
| N1 resolve_scope | main_context | cwd | canonical scope | 已登记 route → canonical route → canonical cwd |
| N2 ensure_daemon | client | scope | daemon ready | 不可用 → 阻塞终点 B1 |
| N3 collect_candidates | adapter | env + scope | candidates | 锚点与端点候选；环境变量只作 locator |
| N4 collect_evidence | providers | candidates + challenge | envelopes | 锁外采集，bounded timeout |
| N5 evaluate | evaluator | envelopes + policy snapshot | decision | §3.2 判定顺序 |
| N6 cas_commit | sink | decision + expected_revision | binding/generation | 幂等 CAS；revision 变化 → 丢弃旧 proof 重采 |
| N7 emit_outcome | main | 结果 | 快照 / 终态 | 成功写 snapshot，失败写 §3.3 终态 |

边：N1→N2→N3→N4→N5→N6→N7；N5 的拒绝/冲突/Unproven 分支直接到 N7；N6 的 CAS 失败回到 N4（有限次数）。

### 5.3 F2 手动裁决恢复 DAG

| 节点 | owner | 输入 | 输出 | 说明 |
|---|---|---|---|---|
| A1 parse_declaration | CLI | `--worker` + `--scope` | declaration | 缺一即拒（M2）；T0 期入口是 `collab context --worker`，`--scope` 派生自 cwd/route |
| A2 load_target | identity | declaration | target principal/binding | 按路径直读，不做 liveness probe（I4） |
| A3 collect_inherited | state | target principal | 订阅/lease 清单 | §3.4；用于 receipt 与提示 |
| A4 write_receipt | identity | declaration + target + 继承清单 | receipt | M3；TTL 由配置给定 |
| A5 rebind | sink | receipt + expected_generation | 新 binding | 幂等 CAS；不回退 generation |
| A6 emit_outcome | CLI | 结果 | `ADJUDICATED` + receipt + 继承清单 | 唯一成功终点 |

边：A1→A2→A3→A4→A5→A6。A5 失败（generation 已前进）→ 回到 A4 重写 receipt 或落到失败终态，不静默成功。

### 5.4 终点清单（逐节点，含取消/清理/阻塞）

| 终点类型 | 节点 | 终点定义 | 验收证据 |
|---|---|---|---|
| 成功（F1） | N7 | `ACCEPTED`，binding/generation 已提交，快照可查 | receipt 可查询 + generation 单调 |
| 成功（F2） | A6 | `ADJUDICATED`，receipt 已写且含继承清单 | receipt 可查询 + `collab identity revoke` 可用 |
| 失败（F1） | N7 | §3.3 任一终态码 + repair_required + 命令模板 | 断言终态码与修复入口 |
| 失败（F2） | A1/A5 | 声明不完整 / CAS 冲突 | 断言拒绝原因，不留半写 receipt |
| 取消（F2） | A4–A5 | 操作者中断：receipt 与 rebind 必须**同事务**，不得留半写 receipt | 中断注入测试：无孤儿 receipt |
| 取消（F1） | N4–N6 | 挑战超时/中断：丢弃 proof，不写状态 | 超时注入测试：状态不变 |
| 清理 | receipt/tombstone/archives | TTL 到期 receipt、Principal tombstone、归档记录的 GC | 保留期配置 + GC 幂等；**需显式 GC 授权**，不自动删主体 |
| 阻塞（上游） | N2 | daemon 不可用 | 阻塞终态 B1 + 可重试入口 |
| 阻塞（上游） | N4/N5 | provider timeout / policy snapshot 不可读 | Unknown 终态，不推断"空库"，不影响其他项目 |

**receipt 的消费者（修正原设计的断边）**：`collab context` 输出（展示生效裁决）、`collab identity list`（审计）、`collab identity revoke`（撤销）、GC（清理）。四个消费者都必须实现，否则 receipt 是断边。

### 5.5 与既有 dagpipe 图产物的映射

项目已有图产物与治理入口（`docs/dagpipe/manifest.json`，`dagpipe graph validate` 当前为绿）。本设计对它们的要求：

| 既有图 | 与本设计的关系 | 实现时必须做的动作 |
|---|---|---|
| `appsdk-collab-context`（9 节点：resolve_context_root → ensure_baseline → ensure_daemon → load_identity → verify_token → ensure_registration → restore_default_lease → find_master → emit_snapshot） | 就是 F1 的现状图 | 实现 F1 时同步更新：`load_identity`/`verify_token`/`ensure_registration` 细分/替换为 N3–N6；保持图与实现一致 |
| `appsdk-collab-subscription-lifecycle` | §3.4 订阅归属改变其绑定点（worker_id → principal_id） | 实现 §3.4 时同步更新 |
| `appsdk-collab-appserver-route-repair` | §2.3 binding 基数与 primary/anchor 规范化影响 route repair | 实现 §2.3 时复核 |
| （新增）`appsdk-collab-identity-adjudication` | F2 目前无图产物 | 实现 F2 时新增图并注册进 manifest，通过 `dagpipe graph validate` |

**规则**：图必须与实现同一次提交落地并 validate 通过；图与实现不一致按契约断链处理。

---

## 6. 迁移与实施

### 6.1 阶段 A：局部修复（T0，独立于 quorum 引擎，先行交付）

这四项直接命中"恢复不可靠"（R1/R2），且互不重叠。

| 任务 | 文件 | 交付条件 | 测试条件（red test） |
|---|---|---|---|
| T0-1（F-02） | `collab/src/main.rs` 把 `thread_local!` 提到 module scope，getter/setter 共用同一 static；`collab/src/main_context.rs` 只在显式 `--worker` 时置位；核对读写配对 | getter/setter 共用同一 cell；显式 `--worker` 的注册请求 `retire_cross_project_anchor == true`，daemon 退休 foreign route 且当前 scope 取得锚点；F1（无 `--worker`）恒 `false`，跨 scope 仍 fail-closed 到 `IDENTITY_CROSS_PROJECT`（与 `c5007ad` 契约一致） | 单测：setter→getter 同一 cell；单测：gate 仅对 `Some(worker)` 为真；集成（`ProjectRuntimeManager`）：flag=true 时 foreign route 被退休且当前 scope 持有锚点，flag=false 时被拒（**该路径原先零覆盖**） |
| T0-2（F-03） | `collab/src/server/mod_parts/part_07.rs:140-147` 缺 `thread/status/type` 归 `Unknown`，与 `collab/src/identity.rs` 的 `classify_thread_status` 对齐 | 畸形/缺字段响应不再产生 `Present` | 三条断言：缺 status → `Unknown`；`notLoaded` → `Cold`；`systemError` → `Missing` |
| T0-3（F-04） | `collab/src/server/mod_parts/part_04.rs` 删除手写比较，统一调用 `collab/src/adapters/tmux.rs:308-314` 的 `same_pane_route`；退休后置条件不再二次推导 pane 同一性，改为"该 binding 已不是 live route" | pane 同一性判定只有唯一实现、唯一调用点；`pane_pid` 变化不再被误认 | 唯一调用点一条：pane id 相同、`pane_pid` 不同 → 判不同 |
| T0-4（F-08） | `collab/src/adapters/mod.rs:1-2` 注释与实现对齐（AppServer adapter 未退役、非 `cfg(test)`、生产路径在用）；若确实要退役，必须先迁移 `candidate_from_env`/`verify_candidate` 的生产调用点 | 注释、`cfg` 属性、生产调用三者一致 | 针对性检查 + 断言生产调用存在 |

### 6.2 阶段 B：平台级（F1/F2 的完整实现）

T1 新模型（`identity/model.rs`）+ policy validator（拒绝 floor 下调）→ T2 evidence providers（每域独立可测）→ T3 evaluator + coordinator（锁外采集、锁内 CAS）→ T4 F2 裁决通道 + receipt + revoke + 四个消费者 → T5 legacy 迁移与双读取 + 订阅改绑 principal → T6 三 store 关联字段统一。

每阶段落地时必须同步更新 §5.5 的图产物并 validate。

### 6.3 legacy 允许动作集（迁移期）

旧档案标记 `legacy-unverified`，动作白名单：

| 动作 | 是否允许 |
|---|---|
| 诊断读（`collab context` 展示、候选清单、receipt 查询） | 允许 |
| 接收已订阅的 mailbox payload（只读） | 允许 |
| 发送消息 / 唤醒 / 投递 | **禁止**，直到重新 challenge 通过 |
| route 建立或刷新 | **禁止** |
| master/role 授予 | **禁止** |
| identity 迁移 | 仅经 migration grant 或裁决 receipt |

不允许的动作必须返回可修复终态（携原因 + `repair_required`），不得静默降级成 mailbox-only。

---

## 7. 验收矩阵

| 类型 | 检查 | 正向/反向验收 |
|---|---|---|
| 接线（T0-1） | `retire_cross_project_anchor` 的置位条件 | 修复前恒 `false`（反向）；修复后仅显式 `--worker` 为 `true`，F1 恒 `false` |
| 单测（T0-1） | getter/setter 同一 static | setter(true) 后 getter() 为 true |
| 集成（T0-1） | daemon 退休 foreign route | flag=true：foreign route 消失且当前 scope 持有锚点；flag=false：拒绝且不退休 |
| 黑盒 AppServer（T0-2） | 缺 `thread/status/type` | 必须 `Unknown`；不得 `Present` |
| 黑盒 tmux（T0-3） | pane id 相同、`pane_pid` 变化 | 判不同 pane；唯一调用点一条 |
| 契约（T0-4） | 注释/`cfg`/生产调用一致 | 三者一致；生产调用存在 |
| 恢复判定顺序 | §3.2 的 R-1..R-8 | 每行一条：构造该条件，断言终态码唯一且优先级正确；R-6 不得越过 R-1/R-2 |
| 失败终态 | §3.3 | 每个终态码都携 reason + repair_required + 命令模板 |
| 不变量 | I1–I8 | 各一条测试；删除实现必须变红 |
| 裁决约束 | M1–M7 | 缺 `--scope` 拒绝；非交互入口无法触发；`revoke` 可撤销且不回退 generation；不得写 grant（M1–M7 属 T1；T0 的裁决入口只有 `--worker`，见 §3.2 T0 实现状态） |
| 订阅归属 | §3.4 | 恢复同一 principal 订阅延续；顶替时 receipt 列出继承清单；`--no-inherit-subscriptions` 生效 |
| 零锚点 | R4 | 有 tmux 走 pane 锚点；无 tmux 显式报错，不静默 mint |
| 证据域 | D1–D5 | 同进程多字段只算一组；`min_independent_groups=1` 被 validator 拒绝 |
| 取消 | §5.4 | 中断注入：无孤儿 receipt、状态不变 |
| 清理 | §5.4 | TTL 到期 GC 幂等；不自动删主体；需显式 GC 授权 |
| 阻塞 | §5.4 | daemon 不可用 / provider timeout 收敛到阻塞或 Unknown 终态，不影响其他项目 |
| 图产物 | §5.5 | 实现落地时图与实现一致，`dagpipe graph validate` 通过 |
| 回归 | 既有 identity/route/subscription 测试 | 全绿；legacy 兼容读旧字段 |

---

## 8. 非目标与留白

**非目标**：DSH 协议实现（无 runtime adapter，保持 `enabled:false`，激活门禁见外部设计稿 §8.5）；AppSDK `rust/` 三 store 的重构（只定义关联字段与验收终点）；跨 scope migration grant 的签发机制（在定义前由 F2 承担）。

**留白（需用户确认）**：
1. `AdjudicationReceipt` 的 TTL 默认值与保留期。
2. `--no-inherit-subscriptions` 是否作为默认行为（当前设计：默认继承但必须打印清单）。
3. AppServer adapter 的真实状态（退役 vs 生产在用）——**已确认：生产在用**（`verify_candidate`、`read_thread_status`、`archive_thread`、`start_thread`、`immediate_notify` 均在生产路径），T0-4 已按此对齐注释。
4. `pane_pid` 复用语义的跨平台一致性（已列为 T0-3 的必测项）。

### 8.2 T1 缺口（独立 review 证据，必须在 T1 关闭）

| # | 缺口 | 证据 | 关闭条件 |
|---|---|---|---|
| G-1 | 自动路径（F1）无法恢复跨 scope 记录 | `identity.rs::identity_by_current_anchors_same_scope_at` 在发请求前就 bail `IDENTITY_RESTORE_CROSS_PROJECT`；而 daemon 侧的 dead-only retire 对同 pane 的 foreign duplicate 无解——它探测到的是**同一个 live pane**，永远不判 Dead | F1 实现 dead-only 判定并放开客户端 same-scope guard；补 unnamed dead 跨 scope 的 CLI→server 回归 |
| G-2 | 裁决入口未强制 `--scope`，也无 durable receipt（M2/M3） | `Cmd::Context` 只暴露 `--worker`（`main.rs:170-173`），scope 由 cwd/route 派生；`collab identity adjudicate` 不存在 | 实现 §5.3 F2 DAG 的 A1/A4 节点，强制 M2/M3 |
| G-3 | 退休不具事务性，且无 tombstone | `retire_cross_project_anchor_candidate` 在注册被接纳前就 commit `GlobalCurrentThreadRouteRetired`；后续失败时 `registration_rollback_state` 只快照当前 scope/worker 的 binding，不恢复被退休的 foreign route；退休不写 tombstone，compaction 后 replay 的 `restore_unique_current_thread_routes_from_bindings` 可能复活该 binding | retire 与 replacement binding 原子提交（含 tombstone / archived receipt），或失败时快照并恢复；补失败注入与 replay/compaction 回归 |

G-1/G-2/G-3 都是 T1 的关闭项，不阻断 T0 已交付的机械修复；但它们决定了"任何情况下都能恢复"这一目标尚未达成。
