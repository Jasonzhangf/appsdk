# Collab 身份报告与恢复：最短路径

**状态**：设计 rev 3。rev 1 经独立 reviewer A 判 FAIL；rev 2 逐条修订并经**独立 reviewer B 判
PASS**（7 项全部 resolved，5 条 advisory，0 blocking）；rev 3 采纳该 reviewer 的精确化修订，并加入
能力确认阶段实测发现的第二根因 D2。D2 子句由**独立 reviewer C** 单独复核判 **PASS**（2 条前置条件
已并入本设计）。rev 3 全文另经**独立 reviewer D** 按 (a)–(n) 逐项复核：实质结论 (a)–(m) 全部 PASS；
(n) 判出 4 项**文档缺陷**（状态行过时、reviewer 归属混淆、§8 两行不完整/不可核验），已在本版逐条
修正。逐条回应见 §6。
**当前阶段**：设计 rev 4。C1–C5 已按 rev 3 实现，定向红测先失败、绿测后通过（证据见 §8）。随后
**真实环境黑盒回归**在 pane `%4` 上暴露第三根因 **D3**：孤儿陈旧 claim 的代际高于新注册，写路径
被 `RuntimeBindingTombstone::new` 拒绝，且该次失败会让 daemon **无法启动**（见 §2 D3、§8）。因此
C1–C3 **不能单独交付**——它们在 `%4` 上把"一个 pane 失败"升级为"整台 host 不可启动"。D3 的唯一
owner 与最小修复边界见 §2，变更项为 §7 的 C8。**过程披露（须如实记录）**：C8 的编码前设计准入
review 期间发生了两个失败轮次（B1 与 B2 各一轮），第二轮 PASS 后我开始写产品代码；但另有两位
reviewer 针对**同一期间被修改中的文档修订**在更晚时刻才回报，因此"C8 写码时准入已冻结"这一前提
**未严格成立**。补救：全部修订完成后，在**冻结的最终候选**上重跑独立 review（§6），并以上一轮
reviewer 指出的问题逐条修正为准（§6 记录 B1/B2/B3 的逐条回应）。
**产品代码改动范围**：`collab/src/server/mod_parts/part_04.rs`（错误分类）、
`collab/src/server/identity_context.rs`（容忍与凭证守卫）、
`collab/src/server/global_state_impl_part2.rs`（D3 孤儿判据），以及对应测试。
**基线**：`c3c0c8df`（与 `origin/main` 同步）。
**关联产物**：
- `docs/dagpipe/collab-context.graph.json`（F1，本次升到 v0.8.0）
- `docs/dagpipe/collab-master-authority.graph.json`（F2，本次新增 v0.1.0）
- `docs/dagpipe/manifest.json`、`rust/src/dagpipe.rs`（图注册表）
- 上游语义：`docs/design/collab-identity-minimal-interaction.md`（交互契约）、
  `docs/design/collab-anchor-restore-model.md`（锚点阶梯）、
  `docs/design/collab-pane-route-ownership-20261006.md`（一个 pane 一个 binding）

**非范围**：dsh gateway 自身实现；AppSDK `rust/` 三 store；`collab reset`。

---

## 1. 用户契约（逐条编号，后续章节必须可追溯）

| 编号 | 需求（来源：用户明确要求） | 落点 |
|---|---|---|
| R1 | **daemon 默认自动顶掉冲突** | §4 F1 的 `identity_gate`：pane 被陈旧/他人 claim 时默认顶掉，不 fail-closed |
| R2 | **`collab context` 依据当前 tmux pane 自动返回身份** | §4 F1；CLI 自动观测当前 pane，无参数 |
| R3 | **冲突时一条命令即自动恢复** | §4 F1（`collab context` / `collab init` 共用同一身份门）+ §5 F2（`collab master promote --approval` 恢复权威） |
| R4 | **agent 不再需要自己查 identity/route/archive** | §3.3：agent 可见动作只有一条命令；`who`/`route`/archive 不在链上 |
| R5 | **同步改写 collab skill 说明这条短流程** | §7 skill 改写清单 |
| R6 | 不静默失败、不未经授权破坏、无证据不宣称完成 | §3.4 失败终止态；§8 验收证据 |

**硬约束**：控制面与业务 payload 物理隔离；错误显式暴露；禁止无条件 fallback 与双路径补偿；
项目代码修改在独立 worktree；不得用源码测试替代真实入口黑盒回归。

---

## 2. 实测缺陷（2026-10-07，真实环境）

命令与结果（`TMUX=/private/tmp/tmux-501/default,6911,0`，真实 tmux server 6911）：

| pane | cwd | `collab context` 结果 |
|---|---|---|
| `%2`（routecodex-2） | `/Users/fanzhang/Documents/github/routecodex` | **失败**：`ROUTE_RESOLVE_INVALID: current route state for App Server thread %2 references a missing runtime binding` |
| `%4`（appsdk-2） | `/Users/fanzhang/Documents/github/appsdk` | **失败**：同上，thread `%4` |
| `%5` / `%6` / `%7`（routecodex-3/4/5） | routecodex | **成功**，返回完整身份快照 |
| 无 tmux 的 DSH shell | appsdk | **失败**：`TRANSPORT_NONE: no reachable App Server, tmux or dsh candidate was supplied` |
| routecodex | — | `collab master status` → `status: unknown`，`recorded_worker_id: codex-01a0cd77-8c40-7a32-85a8-f5d191261b8d` |

**根因 D1（读路径）**：`collab/src/server/identity_context.rs::reconcile_identity_context` 在
`resolve_route_by_tmux_endpoint` 上只把 `ROUTE_RESOLVE_NOT_FOUND:` 当作"没有可用 route"，
其余错误一律 `return Err(...)`。陈旧 host route 索引项（host 索引有 route、其 binding 已不在项目
runtime 中）产生的是 `ROUTE_RESOLVE_INVALID:`，于是整条 bootstrap 在**写路径顶掉 pane 之前**
就中止。

**根因 D2（凭证守卫）**：即使读路径不再中止，`codex-%2` 仍会在 Register 被拒。证据：

| 事实 | 证据 |
|---|---|
| `~/.collab/identities/codex-%2/identity.json` 是**不完整草稿**（无 `runtime`、无 `transport`），token `7b7575c3…` | 读文件；mtime Oct 6 08:20 |
| 该 token 在 routecodex 与 appsdk 的任何 journal 中**出现 0 次** | `grep -c 7b7575c3… <journal>` = 0 |
| routecodex runtime 的持久记录里 `codex-%2` 的 token 是 `5cbd484b…` | routecodex journal `Registered`（唯一一条） |
| routecodex runtime **在线持有** `codex-%2`（49 个 worker），appsdk runtime 只有 1 个 | `collab who` 分别从两个项目目录运行 |
| 生产代码只有 `persist_registration_at` 写身份文件，且它**总是**同时写 `runtime` 与 `transport` | `grep -rn "write_identity(" collab/src` |
| 阶梯对无匹配锚点会**沿用草稿的 token** | `collab/src/identity_resolver.rs:152-163` |
| 该守卫因"存在任意文件"而提前返回，于是草稿 token 被送到 Register | 修复前 `collab/src/server/identity_context.rs:209-213`（`read_persisted(..)?.is_some()`）→ 修复后 `:222-226`（`is_some_and(\|p\| p.runtime.is_some())`） |

因此 `Register` 命中 `validate_wire_runtime_binding` 的 `token_mismatch`
（`collab/src/server/mod_parts/part_10.rs`）→ `TOKEN_MISMATCH`，agent 仍然被卡住。

守卫的文档意图是"**已持久化的凭证**不被替换"（修复前 `:200-202`，修复后 `:209-215`："A committed
registration can outlive its local receipt file. Recover that same credential from the reducer only
for the proven anchor. A **committed** persisted credential is never replaced, even when it is
rejected."）。一个没有 `runtime` 的草稿
**不是**凭证，所以把"文件存在"当作"凭证存在"与它自己的契约不符。修复就是让守卫判断"是否已存在
**已提交的**本地凭证"，而不是"是否存在文件"。

**D2 的可见顺序与来源（review 结论）**：D2 只在 D1 修好后才可达——当前代码在
`identity_context.rs:70-78` 就先中止于 D1 的陈旧索引错误（实测 `%2`/`%4` 均如此），根本走不到
`Register`。修好 D1 后，`%2` 才会用草稿 token 走到 `Register` 并命中 `TOKEN_MISMATCH`。
**该草稿不是当前契约可产生的状态**：HEAD 上唯一的身份文件写者 `persist_registration_at`
（`identity.rs:591-610`）总是同时写 `runtime` 与 `transport`；产生"裸草稿"的旧写者已在
`761c43a3`（2026-10-06 18:38 -0700）被删除，而两个裸草稿文件的 mtime 是 08:20 / 08:22 -0700，
早于该重构。所以本项按**遗留状态处理**：它真实存在并确定性阻断真实命令，且守卫与自己的文档契约
相矛盾，因此按谓词缺陷修复（只收窄一个过宽的早退，不新增校验层、状态机或 fallback）。
**替换审计**：修复后本地裸草稿会被 reducer 中该锚点的已提交凭证覆盖，journal 会记录这次
`Register`；`runtime` 存在的凭证仍然一律不被替换（守卫第一项不变）。

**根因 D3（写路径的代际单调性把"陈旧索引"升级为致命错误与 host 级不可重放状态）**：修好 D1 后
`%4` 不再中止于读路径，而是前进到写路径并以 `ROUTE_TRANSITION_DURABILITY_FAILED` 失败（真实环境
实测，见 §8）。证据：

| 观测 | 证据 |
|---|---|
| host 索引对 pane `%4` 持有 **appsdk** scope 的 `binding-codex-_4` gen 7 | routecodex journal 第 1043 行 `GlobalCurrentThreadRouteSet` |
| 该 binding 在**本 reducer 的** `projects` map 里不存在，是**孤儿** | routecodex journal 有 5 条 `GlobalProjectRegistered` 与 60 条 `GlobalRuntimeBound`，**全部**是 routecodex scope，appsdk 均为 0 条；其中 `GlobalRuntimeBound` 提到 `binding-codex-_4` 0 次；两个 journal 的 `GlobalRuntimeBindingRollback` 均 0 次 |
| 新注册只能给出 gen 1 | appsdk runtime 没有该 binding，代际从 1 起算 |
| 于是 reducer 拒绝 | `set_current_thread_route`（`global_state_impl_part2.rs:126-254`）先按 principal 摘除旧 route（候选文件 `:156-169` 的 filter + `retain` **未被 C8 插入影响，行号与 base 相同**；候选 `:170-183` 是 `for old in retired` 块），再为它建 tombstone；`RuntimeBindingTombstone::new`（`global_state_models.rs:468-474`）要求 `rebound_to.endpoint_generation > old.endpoint_generation`，`1 ≤ 7` → `StateError::StaleBinding` |

**后果（实测，必须写进契约）**：该 `GlobalRuntimeBound` 已写入 **appsdk** journal（第 76872 行），
但 host 发布被拒；此后 daemon **每次启动**都在 reconcile 阶段
（`runtime_manager_setup.rs:406-411`）重放同一发布并被同一守卫拒绝，报
`RECOVERY_RECONCILE_REQUIRED: journal reducer failed: … expected generation 7, observed 1`，**daemon
无法启动**。所以 D3 不只是 `%4` 的功能缺陷——它把一次普通身份命令升级成 host 级不可重放状态。
因此 D3 属于本变更的**必做**范围：只修 D1/D2 而留 D3，会让 `collab context` 在孤儿陈旧索引的 pane 上
把整个 host 打挂，这是不可接受的回归。

**D3 的 owner 与最小修复边界**：owner 是全局 route reducer 的 `set_current_thread_route`（tombstone
的单调性判据），**不是**身份门的读路径，也不是 `RuntimeBindingTombstone` 自身的校验；残余拒绝点
（`runtime_manager_setup.rs:427-432` 同地址、`:288-293` pending master grant、`part_04.rs:490-498`
不同 pane）对 `%4` 形状都不触发，所以"D3 唯一 owner"是**就本样本而言**。最小修复：**当被摘除的旧
route 的 `(route_scope, binding_id)` 在**本 reducer 的** `projects` map 里没有任何 binding 时，它就是
孤儿**（已安装的孤儿 route 先被 `:184` 的 `lookup_tmux_route` 命中，随后在 `:291-297` 以
`ROUTE_RESOLVE_STALE_INDEX`（missing runtime binding）失败；若该 route 已摘除而只剩 tombstone，则
`:186-192` 以 `SESSION_THREAD_BINDING_STALE` 失败，无 tombstone 时落到 `:193-198`（地址分支
`:251-255`）的 `ROUTE_RESOLVE_NOT_FOUND`），因此不需要 tombstone；跳过该 tombstone 的
单调性检查即可。判据实现：`next.lookup_binding_for(&old.route_scope(), &old.binding_id).is_none()`
（`global_state_impl.rs:483-491`，`next` 是 `&mut GlobalState`，见 `:955-964`；`retired` 是自有 Vec，
无借用冲突）。

**判据的权威范围（review B1 指出，必须写准）**：判据只能问**本 reducer 自己的** `next.projects`，
**不能**问"所属项目 runtime"。host reducer 连非驻留项目的 project 条目都没有：实测 routecodex journal
的 5 条 `GlobalProjectRegistered` **全部**是 routecodex scope，60 条 `GlobalRuntimeBound` 也全部是
routecodex scope，appsdk 均为 0 条。因此对**非驻留项目**的旧 route，host 无法区分"活"与"孤儿"，
C8 也会跳过它们的 tombstone。这是本修复的**已知边界**，不是隐藏副作用：

- 失去的是**旧的已退役地址上的致命围栏**：该地址从 `SESSION_THREAD_BINDING_STALE`（致命）变为
  `ROUTE_RESOLVE_NOT_FOUND`（被身份门吞掉）。因此对**非驻留项目**，身份门在这条路径上由"中止"
  变为"按锚点重新注册该 pane"。这**不产生任何错误解析**：孤儿地址本身仍在 `part_04.rs:291-297`
  显式失败，tombstone 只增加一条错误，旧 route 在两种情况下都被摘除，且地址重新变活时 tombstone
  照旧被清除（`global_state_impl_part2.rs:219-224`）。
- **代际单调性的真正 owner 是 `bind_runtime`**（`global_state_impl_part2.rs:558-564`）：它在**所属
  runtime** 里照旧拒绝用低代际覆盖活 binding；`validate_binding`（`:731-743`）与 `grant_master`
  （`:769-775`）同理。也就是说，"活 binding 不被降代际覆盖"这条不变式**不由 tombstone 承担**，
  C8 也没有把它交给任何人。
- "一个 pane 一个 binding"由 `:232-239` 的 pane 驱逐保证，与 tombstone 无关。

**对本样本 `%4` 的判定**：host 的 `projects[appsdk]` 里确实没有 `binding-codex-_4`，所以孤儿判据为真，
这正是要修的形状。

**D3 修复对身份门控制流的影响（review 指出，必须显式声明为有意行为）**：跳过孤儿 tombstone 会改变
**身份门**在该 pane 上的控制流，而不只是换一条错误文案。理由：`identity_context.rs:77-83` 只吞
`ROUTE_RESOLVE_NOT_FOUND` 与 `ROUTE_RESOLVE_STALE_INDEX`，其余错误一律致命。逐形状说明：

- **已安装的孤儿 route（本样本 `%4` 的形状）**：`lookup_tmux_route`（`part_04.rs:184`）先命中它，随后
  在 `:291-297` 以 `ROUTE_RESOLVE_STALE_INDEX`（missing runtime binding）失败——这条被身份门吞掉，
  属 C1/C2 的处理范围。修复后写路径成功，该 pane 被新 route 占据，后续 `collab context` 正常解析。
- **已摘除的旧 route**：`:184` 查不到活 route 才在 `:186` 查 tombstone。孤儿修复后不再产生 tombstone，
  于是 tmux 分支落到 `:193-198` 的 `ROUTE_RESOLVE_NOT_FOUND`（地址分支落到 `:251-255`），被身份门
  吞掉 → `collab context` 继续走锚点阶梯并重新注册该 pane。**这正是本任务要的行为**（"daemon 默认
  自动顶掉冲突"、"一条命令即自动恢复"）。

边界与代价（§8 各行的措辞必须与此一致）：

- **驻留项目**（本 reducer 持有该 binding）：**活** binding 的 tombstone 照旧建立，因此**活的**已退役
  pane/地址仍然得到致命的 `SESSION_THREAD_BINDING_STALE`（`part_04.rs:186-192` / `:232-250`）；既有断言
  `host_route_registry_tests/part_02.rs:532`、`:620` 覆盖这条路径。
- **非驻留（split）项目**：host reducer 没有该项目的 project/binding 条目，判据对**每一个**该 scope 的
  退役 route 都为真（活与孤儿无法区分），因此 tombstone 一律跳过，旧的已退役地址得到被吞的
  `ROUTE_RESOLVE_NOT_FOUND` 而不是致命的 `SESSION_THREAD_BINDING_STALE`。这是本修复**已知且有意的
  边界**：失去的只是诊断，不产生任何错误解析（孤儿地址本身仍失败于 `:291-297`）；该项目的代际单调性
  由 `bind_runtime` 在**所属 runtime** 内保证。若要连这条诊断也保住，必须让 host 镜像 split 项目的
  binding，或让判据读到所属 runtime——两者都超出本修复边界，且第二个在 host reducer 里根本不可达。

不采用"把 C8 限制为非 tmux 端点"的替代方案，因为本任务的真实失败样本 `%4` 正是 tmux pane，限制后
无法修复目标缺陷。

**被考虑并否决的替代方案（B）**：保留孤儿 tombstone，只跳过 `RuntimeBindingTombstone::new`
（`global_state_models.rs:468-474`）里的代际单调性比较。否决理由：(1) 它要改第二个 owner，为一个只
影响错误文案的产物新增构造变体，违背奥卡姆剃刀；(2) 它会保留"孤儿旧地址 → 致命
`SESSION_THREAD_BINDING_STALE`"，于是只带地址、不带 tmux 事实的调用者仍会在一次 `collab context`
里失败，与用户契约"agent 不需要第二条命令、不参与裁决"直接冲突。C8 只对**本 reducer 无 binding
证据**的旧 route 放弃该信号，驻留项目的活 binding 信号完整保留，因此 A 是正确的最小边界。

**两个 pane 的陈旧索引项形状（实测）**：

| pane | host 索引项 | 缺失的部分 | 需要 |
|---|---|---|---|
| `%2` | `codex-%2`，scope **appsdk**，`binding-codex-_2`，gen 1，`runtime-tmux-6ccbbbb0cbc2e835`，session `01a0e5be-…`（**routecodex** journal 第 1032 行 `GlobalCurrentThreadRouteSet`，即 host 自己的 journal） | 该 binding 不在 **appsdk** runtime（appsdk journal 中 `codex-%2` / `binding-codex-_2` 各 0 次） | D1；另有 D2（本地草稿 token 与已提交记录不一致） |
| `%4` | `codex-%4`，scope **appsdk**，`binding-codex-_4`，gen 7，session/thread `01a0ebcd-…`（routecodex journal 第 1043 行 `GlobalCurrentThreadRouteSet`） | 该 binding 不在 appsdk runtime（appsdk journal 中 `codex-%4` / `binding-codex-_4` 各 0 次） | 仅 D1（`codex-%4` 无本地身份文件，阶梯会新起草） |

**host 身份与索引归属（实测，D1 证据修正）**：活 daemon PID 95259 的 `cwd` 是
`/Users/fanzhang/Documents/github/routecodex`，`lsof -p 95259` 同时打开 routecodex（fd 12w）、
appsdk（fd 14w）与 `Documents/server`（fd 16w）三个 journal，所以**host 就是 routecodex 的
resident reducer**，host 索引由 **routecodex** journal 重放。据此 `%2` 的 host 索引项是
routecodex journal 第 1032 行（scope appsdk），而**不是** appsdk journal 第 11461 行——后者是
appsdk **runtime 自己**的索引项（agent `codex-01a0cd77-…`，scope routecodex，gen 22），与本次
请求无关。判据：`%4` 的 route set 只存在于 routecodex journal（第 1043 行），appsdk journal 中
`%4` 出现 0 次，而 host 确实报出 `%4` 的 `ROUTE_RESOLVE_INVALID`，说明 host 重放的是 routecodex
journal。因此实测必须按 **host 自己的 journal / host 索引** 定位陈旧项，不能按某一个项目
runtime 的 journal 归属。

**写路径本身是对的**：`set_current_thread_route`
（`collab/src/server/global_state_impl_part2.rs:232-239`）的 retain 谓词不含 route_scope/project
过滤，即"一个 pane host 范围内只有一个 binding，后注册者取胜"，并有跨项目/跨 scope 测试
（`collab/src/server/host_route_registry_tests/part_02_part2.rs` 的
`cross_scope_pane_claimant_takes_the_pane_without_fencing_the_project_route`）。同一 pane 的顶替在
`commit_current_thread_route`（`part_04.rs:478-499`）也是显式允许的。

**结论**：缺陷不在"缺少新命令"，而在两处 owner 与自身契约不符：读路径把可修复的陈旧 claim 当成
不可修复的致命错误（D1），凭证守卫把不完整的本地草稿当成不可替换的已提交凭证（D2）。这两处正是
R1/R2/R4 要求消除的 fail-closed。

---

## 3. 目标契约与最短路径

### 3.1 agent 可见动作（唯一入口）

| 场景 | 命令 | 次数 |
|---|---|---|
| 首次进入 / 日常 / pane 冲突 | `collab context` | 1 |
| AppSDK init 消费者的一次性初始化（同一身份门 + init 投影） | `collab init` | 1 |
| master 权威恢复 | `collab master promote --approval <text>` | 1 |

agent **不执行**：`collab who`、`collab route resolve`、`collab worker status`、
读取 `~/.collab/archives`、`~/.collab/identities`、`routes.jsonl`。

### 3.2 最短有效路径（内部）

```
collab context（1 条命令）
  → resolve_context_root     canonical root（route → cwd → git）
  → ensure_baseline          .agent-collab 存在
  → ensure_daemon            host daemon alive
  → identity_gate            daemon：观测事实 → 锚点阶梯 → 默认顶掉 pane 冲突 → 凭证对账 → Register 提交 → 发布 route
  → env_view                 本地诊断投影
  → emit_snapshot            身份报告
```

**"最短"的判据**（见 §6 问题 3）：
1. 链上每个节点有**唯一 owner** 和**唯一决策**；合并会丢失决策，拆分会产生同 owner 的第二个节点。
2. 没有任何节点只为 agent 的额外探测服务；`who`/`route`/archive 不在链上。
3. pane 冲突在 `identity_gate` 内部默认消解；失败终止态只保留"确实无法唯一归属"的类。

### 3.3 默认顶掉冲突的语义（R1）

pane 是**被拥有的资源**：一个 pane 在 host 范围内只有一个 binding。因此：

**"冲突"的范围（reviewer D advisory）**：R1 的"冲突"只指 **pane 归属冲突**——同一 pane 上存在
陈旧索引项，或存在他人/他项目的 claim。只有这一类默认顶掉。它**不**包括"凭证已被他人占用"
（`TOKEN_MISMATCH`）：那不是资源争用，而是凭证归属问题，按本表末行与 §3.4"真实冲突"保持
fail-closed，由人裁决。本设计与用户"daemon 默认自动顶掉冲突"的措辞在这一范围内一致。

| 观测 | 默认行为 |
|---|---|
| pane 命中已持久化 identity（阶梯第 1 级） | 直恢，沿用 token/runtime/binding |
| pane 的 host route 索引项**陈旧**（引用已不存在的 route/runtime/binding） | 该索引项**不作为恢复依据**；继续锚点阶梯；写路径重新发布本 binding 的 route，并在同一提交内顶掉该 pane 上的陈旧 claim |
| pane 上存在**他人/他项目**的有效 claim | 后注册者按 pane 资源取胜，顶掉旧 claim |
| 同 pane 但**凭证不匹配**且非本人 | 拒绝（`TOKEN_MISMATCH`），不顶替凭证 |
| 同一锚点匹配多条 / 跨项目且无法唯一归属 | 保留显式错误（见 §3.4），由人裁决 |

### 3.4 终止态（invocation-local，不产生 pending 记录）

| 终止态 | 触发 | 显式错误 | agent 动作 |
|---|---|---|---|
| 成功 | 身份已建立/恢复并发布 route | — | 读快照执行 `operations` |
| pane 冲突已顶掉 | host 索引项陈旧或他人 claim | — | 无（默认路径已完成） |
| 缺事实 | AppServer 路径缺 `session_id`/`thread_id`/`endpoint`/`namespace` | `IDENTITY_INFORMATION_REQUIRED` + `required_fields` | 一次 `collab context --provide` |
| 无锚点 | tmux/codex/dsh 三种锚点全缺 | `TRANSPORT_NONE` | 换到项目 main 的 tmux pane 再跑一次 |
| 陈旧地址 | 该地址已被 retire 且有 `reboundTo`（**驻留项目**：本 reducer 持有该 binding） | `SESSION_THREAD_BINDING_STALE` | 用当前地址重跑；不复活旧 route |
| 陈旧地址（**非驻留项目**） | 该地址已被 retire，但本 reducer 无该 binding 证据（§2 声明的 C8 边界） | `ROUTE_RESOLVE_NOT_FOUND`（被身份门吞掉） | 无（身份门按锚点重新注册该 pane；诊断信号在这一情形下不可得） |
| 存活未知 | pane/dsh 探活不确定 | `ROUTE_RESOLVE_UNKNOWN` | 稍后重跑；不猜测 |
| 真实冲突 | host 索引与 runtime binding 不一致（非陈旧） | `ROUTE_RESOLVE_INVALID`（`:300` 类） | 保留原错并上报，不自动顶替 |
| 身份验证失败 | AppServer 身份校验失败 | `ROUTE_RESOLVE_INVALID`（`:361` 类） | 保留原错并上报 |
| 无法唯一归属 | 歧义 / 跨项目 | `IDENTITY_RESTORE_AMBIGUOUS` / `IDENTITY_RESTORE_CROSS_PROJECT` | 保留原错，由人裁决；**默认路径与恢复入口都不覆盖** |

**rev 1 的错误已修正**：rev 1 声称 `collab init` 能"一次恢复"歧义/跨项目终止态。这是假的：
`Cmd::Init`（`collab/src/main.rs:383-419`）与 `Cmd::Context` 走同一个
`identity_context_response` → 同一个 `Req::IdentityContext`，因此同样的
`IDENTITY_RESTORE_AMBIGUOUS` / `IDENTITY_RESTORE_CROSS_PROJECT`。rev 2 不再声明该能力，
歧义/跨项目保持 fail-closed，与 `collab-anchor-restore-model.md` §2 level 3 及
`collab-identity-minimal-interaction.md:18-20` 一致。

**R3 的准确含义**：用户要求的"冲突"是 **pane 被陈旧/他人 claim** 这一类，它在默认路径
（`collab context`，1 条命令）内自动消解，不需要第二个命令；`collab init` 走同一身份门，
因此同样是 1 条命令。master 权威冲突由 F2 的 1 条命令消解。

---

## 4. F1：`appsdk-collab-context` v0.8.0（身份报告）

**单源**：`context_request`。**单汇**：`state_snapshot`。

| # | 节点 | operator | 输入 arc | 输出 arc | owner |
|---|---|---|---|---|---|
| 1 | `resolve_context_root` | `appsdk.collab_context.resolve_root` | `context_request` | `resolved_scope` | CLI route resolver（`Scope::resolve` / `canonical_route_for_cwd`） |
| 2 | `ensure_baseline` | `appsdk.collab_context.ensure_baseline` | `resolved_scope` | `baseline_ready` | CLI baseline（`scope::init`） |
| 3 | `ensure_daemon` | `appsdk.collab_context.ensure_daemon` | `baseline_ready` | `daemon_ready` | CLI daemon lifecycle（`client::ensure_server`） |
| 4 | `identity_gate` | `appsdk.collab_context.identity_gate` | `daemon_ready` | `identity_result` | daemon identity context + resolver + Register 事务 |
| 5 | `env_view` | `appsdk.collab_context.env_view` | `identity_result` | `env_projection` | CLI 本地诊断投影 |
| 6 | `emit_snapshot` | `appsdk.collab_context.emit_snapshot` | `env_projection` | `state_snapshot` | CLI 身份报告 |

拓扑与 v0.7.0 相同；v0.8.0 变更的是 `identity_gate` 的**语义**（本次代码修复落地后为真）：

1. 读路径不再把**陈旧 host route 索引项**当致命错误（§2 根因 D1）；该索引项不作为恢复依据，
   锚点阶梯继续，写路径重新发布本 binding 的 route 并在同一提交内顶掉该 pane 上的陈旧 claim。
2. 凭证守卫只在**已提交的本地凭证**存在时跳过恢复（§2 根因 D2）；不完整的本地草稿不是凭证，
   因此会从 reducer 的已提交 worker 记录按锚点恢复同一 token，而不是把草稿 token 送去 Register。
   若 reducer 中没有该锚点的已提交记录，草稿 token 照旧保留（"never remint a stored credential"
   不变）。
3. 只放宽 D1 的陈旧索引这一类；真实冲突、身份验证失败、存活未知、陈旧地址、歧义保持显式错误
   （§3.4）。D2 不放宽任何错误：它把"拒绝"变成"按锚点恢复同一凭证"，若 reducer 中不存在该锚点的
   已提交记录，行为与今天完全一致。
4. 写路径（`set_current_thread_route`）在摘除同一 principal 的旧 route 时，只为**本 reducer 的
   `projects` map 里仍能查到该 binding** 的旧 route 建 tombstone；**孤儿**旧 route（本 reducer 查不到
   该 `(route_scope, binding_id)`）不需要 tombstone，也不参与代际单调性检查（§2 根因 D3，含其中
   声明的**非驻留项目边界**）。只有这样，"后注册者顶掉该 pane 上的陈旧 claim"在孤儿情形下才真正
   可达；"活 binding 不被降代际覆盖"由所属 runtime 的 `bind_runtime` 承担（不在本路径），"地址变更
   必须升代际"与"一个 pane 一个 binding"两条不变式保持不变。D3 与 D1/D2 同属 `identity_gate` 的
   读/写路径 owner，仍**不新增节点、不新增图**。

**不拆 `identity_gate` 的理由（rev 2 修正措辞）**：rev 1 称"选择、对账、binding/route 提交、
租约是同一个 daemon 事务"，这不准确——持久化是**两次提交**：`Register` 提交
`ProjectRegistered` + `RuntimeBound`（`collab/src/server/state.rs`），随后
`commit_current_thread_route`（`part_04.rs:418-562`）单独提交 host route。准确表述是：

- 整个准入由 `identity_gate` 互斥锁串行化（`collab/src/server/identity_context.rs:13`），
  所以拆分不会带来并发收益；
- 两次提交之间有**显式回滚协议**：`Definite` 发布失败提交
  `GlobalRuntimeBindingRollback` 恢复 previous binding/grant/worker/subscriptions
  （`part_04.rs:537-555`）；`Ambiguous` 失败显式报告"结果未知"并要求按流程重启
  （`part_04.rs:556-560`），不伪装成功。**精确边界（review 复核）**：生产路径上每种
  `JournalError` 都映射到 `Ambiguous`（`part_04.rs:519-535`），`Definite` 只由
  `#[cfg(test)]` 注入的失败构造（`:500-512`）；因此这条回滚分支是**测试覆盖的协议**，
  而不是生产路径上的活跃安全网，生产上真正保证"不静默中间态"的是
  `identity_context.rs:109-120`——`!receipt.ok` 时在 `persist_registration_at` **之前**返回，
  所以不会留下本地 receipt；
- 拆成"解析身份"和"提交注册+发布 route"会产生同 owner 的第二个节点，并把上述回滚协议
  切成两个入口。因此保持一个节点，理由改为"互斥串行的准入 + 两次提交的显式回滚协议"。

**`env_view` 不并入 `emit_snapshot`（review 裁决为 ADVISORY，保留独立）**：
`context_env_view`（`collab/src/main_context.rs:115-134`）是 CLI 私有投影，唯一决策是
按名单/前缀选择变量并丢弃凭证形名字（`TOKEN`/`KEY`/`SECRET`/`PASSWORD`/`CREDENTIAL`）。
合并不会丢决策，但独立节点让它可被单独测试（凭证不外泄是安全属性），因此保留。

**`ensure_baseline` 与 `ensure_daemon` 保持分开（review 裁决 PASS）**：owner 与失败模式不同
（`scope::init` 的基线/playground 拒绝 vs `client::ensure_server` 的 daemon 生命周期），
终止态也不同。

---

## 5. F2：`appsdk-collab-master-authority` v0.1.0（master 权威恢复）

**单源**：`authority_request`（`collab master promote --approval <text>`）。
**单汇**：`authority_receipt`。

| # | 节点 | operator | 输入 arc | 输出 arc | owner |
|---|---|---|---|---|---|
| 1 | `resolve_authority_scope` | `appsdk.collab_authority.resolve_scope` | `authority_request` | `authority_scope` | CLI bootstrap：`Scope::resolve` + `me()`，即 F1 `appsdk-collab-context` 的终态（已注册且可用的身份） |
| 2 | `transfer_master_authority` | `appsdk.collab_authority.transfer_master` | `authority_scope` | `master_authority` | daemon `handle_master_promote`（`collab/src/server/mod_parts/part_07.rs:1224-1267`） |
| 3 | `emit_authority_receipt` | `appsdk.collab_authority.emit_receipt` | `master_authority` | `authority_receipt` | CLI 回执 |

**节点 1 复用 F1，不复制 F1 的节点**：`collab master promote` 先 `me(&scope)`
（`collab/src/main.rs:652-657`）→ `identity_context_response` → F1 的 `identity_gate`，
随后才是本图的节点 2。因此 F1 是 F2 的**上游依赖边**，不是 F2 内部节点；把
`resolve_context_root`/`ensure_baseline`/`ensure_daemon`/`identity_gate` 再画一遍会复制 4 个
节点而不新增决策。

**节点 2 的语义（一个 daemon 事务，显式授权）**：
- 要求调用者已是该 route_scope 下的已注册 worker 且 token 匹配；
- 要求 `--approval` 非空；该显式批准就是本次转移的全部授权；
- 用 `master_authority_transfer_events` 替换已记录的 incumbent，不查询其存活
  （pane 是地址不是存活凭证）；
- 无 `--approval` 时不触碰 master 权威。

**为什么 F2 是权威图而不是身份恢复图（rev 2 修正）**：rev 1 把 F2 画成"身份恢复"，与 F1
`identity_gate` 重合，被独立 review 判 FAIL。修订后 F2 只承载**唯一不同的对象流**：master
权威的转移（`Req::MasterPromote`），它有独立 owner、独立决策（显式批准）、独立终态，
且与 F1 是上下游关系。

---

## 6. DAG 逻辑审核记录（独立 reviewer：rev 1 结论 → rev 2/3 处置）

**rev 1**：独立 reviewer A 判 **FAIL**（2 项 blocking）。
**rev 2**：逐条修订后由**独立 reviewer B 判 PASS**（7 项全部 resolved，5 条 advisory，0 blocking）。
**rev 3**：采纳 rev 2 reviewer 的精确化修订，并加入能力确认阶段实测发现的 D2；随后由**独立
reviewer C** 专门复核 D2 子句（判 PASS，2 条前置条件），再由**独立 reviewer D** 对 rev 3 全文按
(a)–(n) 逐项复核。**reviewer D 判 FAIL，但失败项全部是本文档自身的状态陈述缺陷（(n) 的 4 项）；
实质设计检查 (a)–(m) 全部 PASS**，包括：陈旧索引类别的边界正确、重命名影响面仅一处断言、C2 未吞掉
任何真实错误类、D2 真实且无路径接受草稿 token、C3 最小且"不重发凭证"不变、C3 与 D2 同处
`identity_gate` 互斥区、修好 D1+C3 后 `%2` 可恢复、`%4` 仅需 D1、F2 成立且 3 节点最小、治理与
SESE 校验通过、§9 旧契约无残留矛盾。reviewer D 的 4 项文档缺陷已在上一段与§8 中逐条修正。
**复核编号说明**：reviewer C 的 D2 子句复核用 **Q1–Q5** 编号；reviewer D 的 rev 3 全文复核用
**(a)–(n)** 编号。两轮编号体系不同且相互独立，不构成同一轮评审的分歧记录。

| # | 问题 | rev 1 裁决 | rev 2/3 处置 |
|---|---|---|---|
| 1 | F1/F2 单源单汇、无环、无孤立节点、每条 arc 恰一个消费者 | PASS | 保持；F2 改为 3 节点后重新校验 |
| 2 | §3.4 终止态可达、无状态机死边 | FAIL | 采纳。删除"`collab init` 可恢复歧义/跨项目"；修正"同一事务"措辞为两次提交 + 显式回滚（§4）；`env_view` 保留 |
| 3 | 最短有效路径（`env_view` 合并 / `identity_gate` 拆分 / `ensure_*` 合并） | PASS + 2 ADVISORY | 采纳 advisory 结论：`env_view` 保留（可独立测试安全属性）；`identity_gate` 不拆但修正理由；`ensure_baseline`/`ensure_daemon` 保持分开 |
| 4 | F2 `recover_identity` 是否与 F1 `identity_gate` 不同 | FAIL | 采纳。删除"身份恢复"F2；F2 改为 master 权威图（§5） |
| 5 | `collab master promote` 是否需独立图 | FAIL | 采纳。`collab master promote --approval` 独立成图，入口事实是 `--approval` |
| 6 | 把整个 `ROUTE_RESOLVE_INVALID` 当"无可用 route"是否吞错 | FAIL | 采纳。**只放宽陈旧索引子条件**：`part_04.rs:265`（unknown route）、`:270`（unavailable runtime）、`:295`（missing runtime binding），改用独立错误码 `ROUTE_RESOLVE_STALE_INDEX:`。复核后确认：从 `resolve_route_by_tmux_endpoint` 可达的 `ROUTE_RESOLVE_INVALID` 站点只有 `:175`/`:177`（id 校验）、`:265`、`:270`、`:282`、`:295`、`:300`、`:312`、`:319`、`:408`、`:414`；`:350`/`:353`/`:361` 与 `:374`/`:377` 在该路径上**不可达**（`:343-347` 与 `:368-372` 先返回 `ROUTE_RESOLVE_NOT_FOUND`）。其中 `:265`/`:270`/`:295` 是仅有的"host 索引指向项目路由表已不再持有的 route/runtime/binding"三处；`:300`（同一 `(route_scope, binding_id)` 内容不一致）是真实冲突，必须保持致命。binding id 是确定性的 `binding-{worker_id}`（`:432`/`:615`），所以其他 principal 的陈旧项只会落到 `:265`/`:270`/`:295`，不会落到 `:300`；实测 `%2`/`%4` 的报错正是 `:295`，被本次修复覆盖。其余保持 `ROUTE_RESOLVE_INVALID:` 显式；`ROUTE_RESOLVE_UNKNOWN`、`SESSION_THREAD_BINDING_STALE`、`ROUTE_RESOLVE_AMBIGUOUS` 不变 |
| 7 | 链上是否迫使 agent 自查 identity/route/archive | PASS（链）/ FAIL（契约） | 采纳。§7 不再把 `collab init` 写成"内部命令不可用"；明确它是与 `collab context` 同一身份门的文档化入口；同时按 §9 更新与之冲突的旧契约表述 |

**缺陷项处置**：
- (a) F1 v0.8.0 描述与代码不一致 → 本次同一变更内落地 `identity_context.rs` 修复，
  描述改为"本次修复后为真"，并修正"同一事务"措辞。
- (b) 注册表/operator 匹配 → PASS，保持。
- (c) §8 可执行性 → 补齐代码变更清单与逐错误类红/绿用例（§8）。

**能力确认阶段新增缺陷 D2**：按门禁要求在写产品代码前确认真实入口与验证边界时，发现 `%2` 的
`TOKEN_MISMATCH` 风险（§2）。D2 不改变拓扑：它与 D1 同属 `identity_gate` 的 owner 与互斥范围，
因此只作为该节点的第二条语义子句进入 F1，不新增节点、不新增图。D2 的修复边界是"守卫的判据要与
它自己的契约一致"，不引入任何 fallback：reducer 中没有该锚点的已提交记录时行为与今天完全一致。

**rev 3 / D2 子句独立复核结论（reviewer C）**：判 **PASS**（Q1–Q5：Q2/Q3/Q4/Q5 PASS；Q1 判
"按字面 FAIL、修好 D1 后 PASS"，即 D2 在 D1 修复前不可达，与本设计一致）。两条前置条件已并入本
设计：(a) `identity_context.rs` 的不变式措辞（修复前 `:200-202`，修复后 `:209-215`）收紧为
"a **committed** persisted credential is never replaced"（C3）；(b) 保留 C5 回归用例"裸草稿 +
reducer 无该锚点记录 → 草稿 token 不变"。
reviewer C 同时指出并把 §2 的 D1 证据修正为"按 host 自己的 journal 归属"（见 §2 修正段），
并给出替换审计风险：修复后本地裸草稿会被已提交凭证覆盖，journal 记录该 `Register`；
`runtime` 存在的凭证仍一律不被替换。

**rev 3 全文复核结论（reviewer D）**：判 **FAIL**，唯一 blocking 项是本文档的状态陈述与
reviewer 归属自相矛盾（(n) 的 2 项 MATERIAL + 2 项 §8 行缺陷），实质检查 (a)–(m) 全部 PASS。
4 项缺陷的处置：状态行改为如实陈述"C1–C5 已实现"；§6 拆分为 reviewer A/B/C/D 四条独立记录并
澄清两轮 (a)–(n) 编号相互独立；§8 "前缀变更影响面"补齐遗漏项并改正 `part_12.rs` 的描述；
§8 "真实环境"行补上可执行命令与期望结果。reviewer D 的 advisory（R1 的"冲突"仅指 pane 归属冲突，
凭证被占用仍 fail-closed）已记入 §3.3/§3.4。

### 6.1 D3/C8 编码前设计准入（rev 4）复核记录

D3 属**编码前设计准入**范围。该范围共发生三轮独立复核，逐条处置如下。

**review B1（对 rev 4 首版，判 PASS）**：要求把判据的权威范围写准。处置：§2 明确判据只能问**本
reducer 自己的** `next.projects`，并把"非驻留项目无法区分活/孤儿"声明为**已知边界**；同时把代际
单调性的 owner 明确为所属 runtime 的 `bind_runtime`，声明该不变式**不由 tombstone 承担**。

**review B2（对 rev 4 中间修订，判 FAIL，3 项 blocking）**：核心指控是判据在 §4 与 §2 中被定义了
两次，且 §4 的规范版（"仍被**所属项目 runtime** binding 支撑"）被本设计自己的样本证伪——按字面实现
**修不好 D3**。处置：
1. §4 第 4 条改为与 §2 一致的实际判据（**本 reducer 的 `projects` map**），并显式指向 §2 声明的
   非驻留边界。
2. 删除"失去的只是诊断"的表述，改为如实陈述：失去的是**旧的已退役地址上的致命围栏**，身份门在该
   路径上由"中止"变为"按锚点重新注册"。
3. §3.4 增加"陈旧地址（非驻留项目）"一行，使终止态表与 C8 边界一致。
4. §8 的 C8 行补上"驻留/非驻留"限定，身份门行的对照明确为**驻留项目**的活 claim（并给出既有断言
   位置 `host_route_registry_tests/part_02.rs:532`、`:620`）。
5. 新增两条回归用例（见 §8）：**非驻留边界**行把该边界显式钉住；**等代际**行钉住"地址变更必须
   升代际"（route 路径 `StaleBinding{7,7}`，ledger 路径 `BindingConflict`）。
6. 新增 §8"残余冲突边界"行，指明读路径 `part_04.rs:300` 的真实冲突仍由 C2 保持致命，C8 不覆盖它。

**review B3（对 rev 4 首版，判 FAIL，4 项 blocking）**：其中两项与 B2 同源（判据双重定义、致命
围栏丢失），已按上条处置。另两项单独处置：
1. **文档事实错误**：原证据行写"该 binding 在任何 runtime 中都不存在"，与同段引用的 appsdk journal
   第 76872 行冲突（该行是**本次失败尝试之后**才写入的 gen 1 `GlobalRuntimeBound`）。已改为按
   **host journal 普查**陈述（5 条 `GlobalProjectRegistered` 与 60 条 `GlobalRuntimeBound` 全为
   routecodex scope，appsdk 0 条）。
2. **准入前提被破坏**：B3 指出"C8 写码时准入未冻结"。该指控成立并已如实记入本文档状态行；补救是在
   **冻结的最终候选**（commit `33d6ab27`）上重跑独立架构 review（§8）。
3. **行号失效**：C8 插入使 `global_state_impl_part2.rs` 中插入点**之后**的行号整体 +13。已逐条重定位
   并核对（每项都已用 base 文件与候选文件逐行比对）：
   `:545-551`→`:558-564`、`:219-226`→`:232-239`、`:942-951`→`:955-964`、`:718-730`→`:731-743`、
   `:756-762`→`:769-775`、`:155-241`→`:126-254`、`:200-202`→`:222-226`（`identity_context.rs`，
   C2 的 +9 使其整体后移）。**插入点之前的行号不变**：`:156-169`（principal 摘除）与 `:155` 的
   `mutate` 起点在 base 与候选**相同**，C8 的 13 行落在 `:194-206`，即原 `:194` 之后；因此
   "`:156-169`→`:169-182`"曾是错的，已改正。`validate_binding`/`grant_master` 的**函数起始行**
   （`:717`/`:753`）与**判据块**（`:731-743`/`:769-775`）是两个不同位置，引用时不得混用。
4. **残余状态**：B3 的"residue"情形（host 索引 gen 7 于地址 A + 所属 runtime gen 1 于地址 B）**不是
   `%4` 的当前形状**（reset 已归档 appsdk journal，appsdk runtime 中不存在 `binding-codex-_4`），
   已在 §8 第 (6) 行写为**须实测确认的前提**，并新增"残余冲突边界"行。

**被否决的替代方案（记录，避免重复讨论）**：让判据去问**所属项目 runtime**，或把 split 项目的
binding 镜像进 host 索引。否决理由：host journal 的回放**不带**项目 runtime，判据在该函数里无法
在回放期获得所属 runtime 的事实；镜像 binding 会引入第二真源并超出本修复边界。若将来要同时保住
非驻留项目的诊断信号，正确做法是在**持有所属 runtime 的写者**（`commit_current_thread_route`）
做判定并把结论**携带在事件里**，使回放保持确定性——这是独立于本修复的架构变更。

---

## 7. 代码变更清单（唯一实现，最小 diff）

| # | 文件 | 变更 |
|---|---|---|
| C1 | `collab/src/server/mod_parts/part_04.rs` | 把 `:265`/`:270`/`:295` 三处陈旧 host 索引条件改发 `ROUTE_RESOLVE_STALE_INDEX:`；其余 `ROUTE_RESOLVE_INVALID:` 站点不动 |
| C2 | `collab/src/server/identity_context.rs` | pane route 证据查询只容忍 `ROUTE_RESOLVE_NOT_FOUND:` 与 `ROUTE_RESOLVE_STALE_INDEX:`（视为"无可用 route 证据"）；其余错误保持致命 |
| C3 | `collab/src/server/identity_context.rs` | 凭证守卫（修复前 `:209-213`，修复后 `:222-226`）改为只在**已提交的本地凭证**存在时跳过恢复：`read_persisted(...)` 结果需带 `runtime`（唯一写者 `persist_registration_at` 总是一起写 `runtime` + `transport`）。不完整草稿照旧保留其 token，除非 reducer 中存在该锚点的已提交记录。**同时**把不变式措辞（修复前 `:200-202`，修复后 `:209-215`）收紧为 "a **committed** persisted credential is never replaced"，使注释与谓词一致（review 条件 a） |
| C4 | `collab/src/server/host_route_registry_tests/part_01.rs`、`.../part_02_tail2.rs` | 更新钉住旧字符串的断言（`part_01.rs:1409-1410`），并新增两类定向用例：陈旧索引被命名且身份门可越过（`stale_pane_route_index_is_named_and_the_identity_gate_bootstraps_over_it`）与真实冲突仍致命（`conflicting_pane_route_index_stays_fatal_for_the_identity_gate`） |
| C5 | `collab/src/server/host_route_registry_tests/part_02_tail2.rs` | 新增 D2 定向用例：不完整草稿 + reducer 中同锚点已提交 worker → 恢复已提交 token；无已提交记录 → 保留草稿 token（回归保护"never remint"） |
| C6 | `collab/skills/collab/SKILL.md`（+ `references/`） | 按 §7.1 改写 |
| C7 | `docs/design/collab-anchor-restore-model.md`、`docs/collab-context-state-machine.md` | 按 §9 更新 `collab init` 的表述（`collab-identity-minimal-interaction.md` 已一致，不改） |
| C8 | `collab/src/server/global_state_impl_part2.rs` | D3 修复：`set_current_thread_route` 在摘除旧 route 后建 tombstone 前，先判断该旧 route 是否被**本 reducer 的 `projects` map** 里的 binding 支撑（`lookup_binding_for`）；**无支撑（孤儿）则跳过 tombstone 与代际单调性检查**。**有意副作用（已声明，见 §2）**：孤儿的旧地址此后报 `ROUTE_RESOLVE_NOT_FOUND` 而不是致命的 `SESSION_THREAD_BINDING_STALE`，身份门因此放行并重新注册该 pane——这正是用户契约要求的"默认自动顶掉冲突、一条命令恢复"。**边界**：判据问的是本 reducer 的 map，所以**驻留项目**的活 binding 信号完整保留，**非驻留（split）项目**因 host 无其 project/binding 条目而一律跳过（活与孤儿不可区分），见 §2 声明的已知边界 |

**C1 的作用域说明（review 复核）**：`resolve_route_by_address` 被
`resolve_route_by_tmux_endpoint` 与 `resolve_route_by_native_thread` 共用，所以这三处改码是
**无条件**的：`Req::RouteResolveNative`（`part_12.rs:137`）也会开始返回
`ROUTE_RESOLVE_STALE_INDEX:`。这是可接受的，因为在两条入口上该条件都仍然是显式错误
（native 入口没有任何容忍分支）；需要同步的消费者只有 `host_route_registry_tests/part_01.rs:1409`
的字符串断言。`main_error_format.rs:2` 只装饰 `ROUTE_RESOLVE_NOT_FOUND:`，`client.rs:926/965`
是客户端 mock 响应测试，两者都不受影响。**不放宽 native 入口**：本任务的契约是"依据当前
tmux pane 自动返回身份"，因此容忍只加在 `identity_gate` 的 pane 证据查询上，不加在共享函数上。

### 7.1 Skill 改写清单（R5）

改写后的措辞必须同时成立：**"`collab init` 不是 agent 的默认入口"** 与
**"`collab init` 与 `collab context` 共用同一身份门，AppSDK init 消费者可以用它"**。
这两句不矛盾，且与 `collab-identity-minimal-interaction.md:219-224` 的
"internal AppSDK init adapter delegating to the same context owner" 一致。改写点：

1. `SKILL.md:520-522`（"`collab context` is the only bootstrap entry; do not run `collab init`"）→
   保留"默认只跑 `collab context`"，把禁止 `collab init` 改为：`collab init` 不是 agent 的
   默认入口，但它与 `collab context` 共用同一 daemon 身份门，所以既有 AppSDK init 消费者
   可以继续用它；禁止的是**单独的 identity/route/archive 探测**，不是这条共用门。
2. `SKILL.md:1023` 表格 "First time in a project" 行的 Never 列 → 把 `collab init` 移出禁列，
   只保留"a separate identity probe"。
3. `SKILL.md:1032-1036` 与 `:1077-1080`（"`collab init` remains an internal compatibility
   adapter, not an agent bootstrap"）→ 保留"不是第二套身份算法/不是 agent 默认入口"，
   补上"它与 `collab context` 共用同一身份门"。
4. `SKILL.md` 新增一句：pane 冲突（陈旧或他人 claim）由 daemon 在默认路径内自动顶掉，
   agent 不参与裁决，也不需要第二条命令；master 权威冲突用一条
   `collab master promote --approval <text>`。
5. 删除 agent 侧的 `who` / `route resolve` / archive 排查步骤与 worker-id 选择指引
   （`SKILL.md:1006-1030` 的 Never 列与 `references/state-paths.md:141`）。
6. 保留 dsh 通道的 gateway 归属说明（身份创建仍属 gateway），并保留
   "歧义/跨项目 fail-closed，由人裁决"（§3.4）。
7. `collab/skills/collab/` 源与 `~/.agents/skills/collab/` 安装字节一致（`install-skills` 校验）。

---

## 8. 验收证据（绑定候选 SHA）

| 项 | 方法 | 期望 |
|---|---|---|
| 定向红测 | `cargo test --bins -- stale_pane_route_index_is_named runtime_less_draft native_thread_route_resolution_ignores_history`，在 C1–C3 **之前**跑 | 3 红 1 绿：陈旧索引用例中止于 `ROUTE_RESOLVE_INVALID`（2 处），D2 用例中止于 `TOKEN_MISMATCH: worker codex-%0 is registered by another token`；回归保护用例本就绿。证据 `~/.collab/runs/collab-identity-shortest-path-20261007/red/red-tests-20261007.txt` |
| 定向绿测 | 同一命令再加 `conflicting_pane_route_index_stays_fatal`，在 C1+C2+C3 **之后**跑 | 5/5 绿（`native_thread_route_resolution_ignores_history…`、`stale_pane_route_index_is_named…`、`conflicting_pane_route_index_stays_fatal…`、`runtime_less_draft_recovers_the_committed_credential`、`runtime_less_draft_keeps_its_token_without_a_committed_record`）。证据 `~/.collab/runs/collab-identity-shortest-path-20261007/red/green-tests-20261007.txt` |
| 边界红/绿 | 放宽类有正/反两例：`stale_pane_route_index_is_named…`（放宽成功、身份门越过）与 `conflicting_pane_route_index_stays_fatal…`（`:300` 真实冲突仍为 `ROUTE_RESOLVE_INVALID`，身份门 fail-closed）。`ROUTE_RESOLVE_UNKNOWN`、`SESSION_THREAD_BINDING_STALE`、`ROUTE_RESOLVE_AMBIGUOUS` 各保持自身错误码，未进入本次放宽分支（只读核对 `identity_context.rs:70-85` 的匹配臂）。`:175/:177` id 校验边界由既有 `native_thread_route_resolution_ignores_history…` 中 `["", "thread\ninvalid"]` → `ROUTE_RESOLVE_INVALID` 的断言覆盖。`:361`（AppServer 身份校验失败）在 tmux 路径上不可达（被 `:343-347`、`:368-372` 的 `NOT_FOUND` 早返回挡住），**本次不新增用例**，其保持 `ROUTE_RESOLVE_INVALID` 由代码只读核对 | 放宽类成功、显式类保持原错 |
| D2 红测 | 构造"不完整本地草稿 + reducer 中同锚点已提交 worker（不同 token）" | 修复前：草稿 token 被送去 Register；修复后：恢复已提交 token |
| D2 回归保护 | 构造"不完整本地草稿 + reducer 中无该锚点记录" | 草稿 token 被保留（"never remint a stored credential" 不变） |
| D3 红/绿测 (1) | reducer 单测：孤儿旧 route（gen 7，**本 reducer 的 `projects` 中无该 binding**）→ 同 principal/binding 的 gen 1 落在**不同地址** | 修复前 `StateError::StaleBinding{expected_generation:7, observed_generation:1}`；修复后 Ok，孤儿被驱逐，该 pane 只剩一个 claimant，且孤儿地址**没有** tombstone |
| D3 回归保护 (2) | reducer 单测：**活**旧 route（gen 7，**本 reducer 中有** backing binding）→ gen 1 不同地址 | 修复前后都必须 `StateError::StaleBinding{expected 7, observed 1}`（模式见 `global_state_tests.rs:1190-1244`） |
| D3 正常升级 (3) | reducer 单测：活 gen 7 → gen 8 不同地址 | Ok；tombstone 的 `reboundTo` 代际为 8；旧地址解析得到 `SESSION_THREAD_BINDING_STALE` |
| D3 非驻留边界 (4) | reducer 单测：旧 route 的 binding 只存在于"另一个 runtime"（**本 reducer 的 `projects` 里没有该 project/binding**，即 split／非驻留形状）→ gen 1 | 把 C8 的**已知边界**显式钉住：跳过 tombstone 并成功顶替，且该**已退役地址不再得到致命的 `SESSION_THREAD_BINDING_STALE`**。目的是让这条边界可验证、可见，而不是靠推断 |
| D3 单调性 owner (5) | reducer 单测：所属 runtime 内 `bind_runtime` 用 gen 1 覆盖活 gen 7 | 仍 `StateError::StaleBinding`（`global_state_impl_part2.rs:558-564`）——单调性 owner 不在 tombstone |
| D3 身份门行为 | 集成用例（`host_route_registry_tests`）：host 索引中存在**孤儿 tmux claim**（代际高于新注册、本 reducer 无该 binding），身份门按该 pane 取证据 | 门**不**因 `SESSION_THREAD_BINDING_STALE` 中止，而是按锚点重新注册并顶掉该 pane（无 C8 时写路径会以 `StaleBinding` 失败，所以本行确实覆盖 C8，而不只是 C1/C2 的读路径）。对照：**驻留项目**的活 claim 被退役后 `SESSION_THREAD_BINDING_STALE` 仍然致命（既有断言 `host_route_registry_tests/part_02.rs:532`、`:620` 不变） |
| D3 真实入口黑盒 (6) | 真实 tmux pane `%4`（appsdk 项目，host 索引中正是那个 gen 7 孤儿）跑 `collab context`，随后 `collab down`/`collab up` | exit 0，返回身份快照；重启后 daemon 正常启动，不得再出现 `RECOVERY_RECONCILE_REQUIRED: … expected generation 7, observed 1`。**前提（须实测确认，不得假定）**：appsdk runtime 中不存在同 `(route_scope, binding_id)` 的**不同地址** binding；若存在，读路径会先在 `part_04.rs:300` 报 `ROUTE_RESOLVE_INVALID: … conflicts with its runtime binding`，而 C2 刻意保持该错误致命，`collab context` **不会** exit 0，须先 `collab down`/`collab up` 由 reconcile 收敛（见下一行） |
| D3 残余冲突边界 | 集成用例（既有 `host_route_registry_tests/part_02_tail2.rs` 的 `conflicting_pane_route_index_stays_fatal_for_the_identity_gate`）：host 索引项与所属 runtime 的 binding 在**同一 binding id、不同代际**上不一致 | 读路径 `part_04.rs:300` 报 `ROUTE_RESOLVE_INVALID: … conflicts with its runtime binding`，身份门 fail-closed，**不**自动顶替。C8 只对"本 reducer 无 binding 证据"的旧 route 放行，不覆盖这一真实冲突 |
| 真实入口黑盒 | 真实 tmux pane（`%2`、`%4`）跑 `collab context` | 返回身份快照，exit 0；陈旧 claim 被顶掉；`%2` 使用 reducer 中已提交的 token，而不是本地草稿 token；`%4` 的 gen 7 孤儿 claim 被顶掉后 daemon 仍可重启 |
| 真实入口黑盒 | **pane 顶替类**冲突 pane 跑 `collab init`（陈旧 claim / 他人 claim；不是"凭证被他人占用"类） | 一次恢复，输出 AppSDK init 契约字段（`runtime.tmuxEndpoint`/`transport_selected`/`runtimeId`/`appserverId`/`projectRoot`/capabilities/`processId`） |
| 真实入口黑盒 | `%2` 的 D2 场景（本地草稿 token ≠ 已提交 token）跑 `collab context` | 恢复已提交 token 后注册成功；不出现 `TOKEN_MISMATCH` |
| 图门禁 | `dagpipe graph validate`（F1+F2）+ `cargo test --bins dagpipe` | valid / PASS |
| 前缀变更影响面 | `grep -rn ROUTE_RESOLVE_INVALID collab/src` 逐个核对：`part_04.rs:265/270/295` 是**唯一**被改名为 `ROUTE_RESOLVE_STALE_INDEX` 的生产者；`part_04.rs:175/177`（id 校验）、`:282`（无 session）、`:300`（与 runtime binding 冲突）、`:312`（无已选 transport）、`:319`（pane 无 endpoint）、`:408`（无原生 thread）、`:414`（`route.validate`）**保持 `ROUTE_RESOLVE_INVALID`**；`part_04.rs:99-102` 属 `resolve_staged_pane_recovery`，不在本次路径；`host_route_registry_tests/part_01.rs:1409` 是唯一需要改的断言（改为 `ROUTE_RESOLVE_STALE_INDEX`），`:1391` 仍断言 `ROUTE_RESOLVE_INVALID`（id 校验边界）；`main_error_format.rs:2` 只装饰 `ROUTE_RESOLVE_NOT_FOUND`，不受影响；`client.rs:161-172`（`validate_route_resolution`）是**生产**客户端校验：它检查 daemon 返回的 session/thread 是否与请求一致，与 host 索引陈旧无关，保持 `ROUTE_RESOLVE_INVALID`；`client.rs:926/:965` 是 `#[cfg(test)]` 内的断言（mock responder），不受影响；`part_12.rs:115/126/141` 与 `main.rs:583` 各有独立字面量，**未被重命名**，其可见字符串不变 | 影响面已穷举：唯一改动的生产者 3 处、唯一改动的断言 1 处 |
| 真实入口黑盒 | routecodex `collab master promote --approval` | master 恢复为调用者 |
| 安装 | 安装到实际运行位置并 `collab down`/`collab up` | health/runtime 加载新 binary |
| 独立 review | 架构 review（非作者） | PASS |
| 真实环境 | 按下方"真实环境收口命令与判据"执行两条命令 | 每条都有明确结论，判据见下 |

**已收集证据（绑定候选 commit `33d6ab27`，base `origin/main` = `c3c0c8df`）**：

| 项 | 结果 | 证据 |
|---|---|---|
| 定向红测（C8 守卫**缺位**时） | 2 条孤儿用例失败，报错正是生产错误：`StaleBinding { binding_id: "binding-orphan", expected_generation: 7, observed_generation: 1 }` 与 `binding-non-resident` 同形；3 条保护用例仍通过 | `~/.collab/runs/collab-identity-shortest-path-20261007/red/d3-red-tests-20261007.txt` |
| 定向绿测（C8 守卫在位） | 7/7 通过：孤儿顶替、活 binding 低代际仍 `StaleBinding`、活 binding 升代际保留 tombstone（`reboundTo`=8）、非驻留边界显式钉住、`bind_runtime` 单调性 owner、等代际拒绝、身份门孤儿顶替集成用例 | 同上目录 `d3-green-tests-20261007.txt` |
| 全量单测 | `cargo test --bins`：**922 passed / 3 failed / 1 ignored**；3 条失败全部属既有 `client::tests` stale-socket 家族，`--test-threads=1` 下 20/20 通过（base `c3c0c8df` 为 911/3，C8 之前为 916/2），判定为既有并行环境抖动，非本次回归 | `.../red/full-suite-postC8-20261007.txt` |
| 图门禁 | F1 `valid DAG: appsdk-collab-context@0.8.0 (6 nodes, 5 edges, 6 waves)`；F2 `valid DAG: appsdk-collab-master-authority@0.1.0 (3 nodes, 2 edges, 3 waves)`；manifest 13 graphs；**在 `rust/` 下**跑注册表门 `cargo test --bins dagpipe` = **12 passed / 0 failed** | `dagpipe graph validate` 输出；`rust/` 注册表门 |
| 构建 | `scripts/build-collab.sh` → `collab_build_version=0.2.0250` | 构建输出 |
| 安装与 digest | `./scripts/install-global-collab.sh` → `version=collab 0.2.0251`，`collab_sha256=8e8d1a06a9dc62571acc67788d06797f39b8228f1ed1bcd719b6d5409f092ce4`，`collab_mcp_sha256=e0f6608b1b12b6d5101be318997d0ef6cb4850b4096ce374e20c3b5929abc195`；`shasum -a 256 ~/.cargo/bin/collab` 与之**逐位相同** | 安装输出 + 本机哈希 |
| 真实入口黑盒 `%2`（驻留项目） | `TMUX=/private/tmp/tmux-501/default,6911,0 TMUX_PANE=%2 collab context` → **exit 0**，`agent_id=codex-%2`、`binding_id=binding-codex-_2`、`endpoint_generation=2`、`registered=true` | `/tmp/pct2b.json` |
| **真实入口黑盒 `%4`（D3 目标形状）** | 同上形式在 appsdk pane → **exit 0**，`agent_id=codex-%4`、`binding_id=binding-codex-_4`、`endpoint_generation=1`、`registered=true`。修复前同一调用产生 `ROUTE_TRANSITION_DURABILITY_FAILED` 并使 daemon 无法启动 | `/tmp/pct4-context.json` |
| **持久性（D3 的核心回归）** | `%4` 写入后 `collab down`/`collab up` → **干净启动**，无 `RECOVERY_RECONCILE_REQUIRED`；`~/.collab/log.txt` 中该错误仍恰好 13 条且全部 ≤ 12:20:10（早于 12:28 的 reset），**无新增**；重启后 `%4` 仍 exit 0（gen 1）、`%2` 仍 exit 0（gen 2），说明顶替可从 host journal 回放 | `~/.collab/log.txt`；`/tmp/pct4b.json`、`/tmp/pct2b.json` |
| MCP 入口 | stdio JSON-RPC `initialize` → `serverInfo {name: collab, version: 0.2.0251}`、`protocolVersion 2024-11-05`；`tools/list` → 33 个工具 | `/tmp/mcp-init.json` |
| master 身份 | `collab master promote --approval "<用户文本>"`（pane `%2`）→ `master=codex-%2`、`mode=user_approved_self_promotion`。`collab master status` 仍报 `status=unknown` 且 `recorded_worker_id=codex-%2`，因为 `live_master_id`（`part_07.rs:403-425`）把 `IdentityPresence::Unknown` 映射为该错误——**契约行为，非本次缺陷** | 命令输出 |
| dsh 会话是否注册 peer | **未注册**（确定结论）：`env -u TMUX -u TMUX_PANE collab context` → `TRANSPORT_NONE: no reachable App Server, tmux or dsh candidate was supplied`；`DSH_SESSION_ID`/`DSH_PROFILE` 存在，但 `$HOME/.dsh` 下无 `*.sock`，故 dsh 锚点当前不可达 | 命令输出 |
| 清理前状态 | 修复前的污染只存在于 `~/.collab/reset.jsonl` 与 `~/.collab/archives/**`（已被 reset 归档），**不在任何 live journal**；live `appsdk/.agent-collab/server/journal.jsonl` 中 `binding-codex-_4` 全部为 gen 1，`~/.collab/routes.jsonl` 已重新登记 appsdk 项目 | 文件核对 |

**真实环境收口命令与判据**：

1. **恢复 master 身份**（在 routecodex 的 master pane 上执行）：

   ```bash
   cd /Users/fanzhang/Documents/github/routecodex && collab master status
   ```

   判据：`status` 不是 `unknown`，且 `recorded_worker_id` 等于该 pane `collab context` 报出的
   `worker_id`。若仍为 `unknown`，按契约在该 master pane 执行一次
   `collab master promote --approval "<原因>"`，再复查同一条命令；仍为 `unknown` 判未收口。

2. **确认 dsh 会话是否已注册 peer**（在非 tmux 的 dsh shell 中执行）：

   ```bash
   env -u TMUX -u TMUX_PANE collab context; echo "exit=$?"
   ```

   判据：**已注册**时返回完整身份快照且 `exit=0`；**未注册**时以
   `TRANSPORT_NONE: no reachable App Server, tmux or dsh candidate was supplied` 失败且 `exit=1`。
   两者都是明确结论——判据是"该命令给出哪一个可观察结果"，不是"必须成功"。

**能力确认**：独立 worktree（已建）、Rust 工具链、已安装 CLI、dagpipe、隔离 tmux CLI/daemon E2E、
真实 tmux server 6911 与 routecodex/appsdk 项目、黑盒入口均可用。缺任一能力或 review FAIL 判
`INCOMPLETE`。

---

## 9. 旧契约表述的更新（避免两个真源）

核查后有两处旧表述与本次契约冲突，必须同一变更内更新：

| 文件 | 冲突表述 | 更新 |
|---|---|---|
| `docs/design/collab-anchor-restore-model.md:98` | "没有 `collab init`" | 改为：`collab init` 不是第二条身份路径，它与 `collab context` 共用同一 daemon 身份门（`identity_gate`），只是为既有 AppSDK init 消费者输出既有响应形状；歧义/跨项目在两条入口上都 fail-closed |
| `docs/collab-context-state-machine.md:187-189` | 把 `init` 与 status/route 一起列为 agent 不得运行的探测命令；"`collab init` 保留为内部兼容 adapter，不是 Agent bootstrap" | 把 `init` 从探测命令列表移除；补上"不是默认入口但共用同一身份门"，并加入 pane 冲突默认顶掉与 master 权威独立命令两句 |

`docs/design/collab-identity-minimal-interaction.md` **无需改动**：它在 "Removal and acceptance"
一节已写明 "Preserve only an internal AppSDK init adapter delegating to the same context owner
if its consumer needs the existing response shape"，与本次契约一致；它也已声明早先草案的
"extra init recovery entry" 被 superseded，这正是 rev 2 删除身份恢复图（F2 改为 master 权威图）
的依据。`docs/design/appsdk-project-integration.md:265-271` 描述 `appsdk init` 以同一环境调用一次
官方 `collab init` 完成 daemon/peer/默认订阅，与本次契约一致，不改。
