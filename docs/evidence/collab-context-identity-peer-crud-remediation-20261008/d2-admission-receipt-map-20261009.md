**状态：READY_FOR_CONTRACT**

本结论仅来自候选 `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a` 的静态源码观察。未运行 daemon、CLI、MCP、测试、socket 或身份动作，不构成行为验证。

**结论**

1. `IdentityContext` 确实在普通 route admission 前被拦截。`dispatch_sync` 先验证 `ProjectContext`，随后立即处理 `Req::IdentityContext`，早于 `route_key`、pending route、`validate_request_context` 和普通 command admission：[part_04.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/mod_parts/part_04.rs:1078)。CLI `me()` 也先调用同一入口，而不是先完成旧 token 认证：[main.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/main.rs:46)。
2. 外层 `IdentityContext` 没有稳定 operation/receipt/query。其 wire 输入只有 `facts`，没有 operation key、approval、target 或 query 语义：[proto.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/proto.rs:468)。成功响应只有 `snapshot` 和本地 `identity_receipt`，后者不是 durable command receipt：[identity_context.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/identity_context.rs:141)。
3. 外层嵌套调用的 `Req::Register` 有自己的 typed receipt；该 receipt 只在内部被读取为注册成功证据，未提升为外层恢复意图的 receipt/query：[identity_context.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/identity_context.rs:109)、[part_06.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/mod_parts/part_06.rs:742)。
4. durable command receipt 已有内部能力，但没有 public query。`GlobalState::lookup_command_receipt` 仅被内部 commit/projection 路径调用，不存在公开 `Req` 变体或 context projection 读取它：[global_state_impl_part2.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/global_state_impl_part2.rs:308)。

**函数级 Owner Map**

| 阶段 | 当前 owner 与调用路径 | 当前证据/边界 | 状态 |
|---|---|---|---|
| host-local admission | `ProjectRuntimeManager::dispatch_sync` → `identity_context` | `ProjectContext` 验证后直接进入 identity gate，绕过普通 route admission | 存在入口；无恢复意图 admission |
| 身份决策 | `reconcile_identity_context` → `resolve_for_daemon_with_route_at` → `reconcile_committed_credential` | 支持 anchor 恢复；本地 `runtime` 已存在时提前返回，旧冲突不进入批准路径 | 缺批准决策与阶段 receipt |
| Register/binding | `dispatch_sync(Req::Register)` → `handle_register_with_app_scope_inner` → `typed_dispatch` → `commit_command_locked` | nested `RegisterWorker` 有原子 command receipt | 已有内部 receipt；未绑定外层 operation |
| route | `commit_current_thread_route` | 独立 route journal；明确错误回滚，append/flush/replay/reducer 错误为 unknown | 独立提交边界；无统一外层 readback |
| credential file | `identity::persist_registration_at` | 位于 Register 成功后、Context 读取前，文件写入不在 Register transaction 内 | 非原子阶段；需外层权威 readback |
| grant | `typed_dispatch` 的 `GlobalMasterGranted` 重发逻辑 | master grant 与 identity binding 是不同 owner；同主体 generation 递增可在注册事务中重发 | 身份恢复不能隐式改变 grant |
| lease | `handle_context` 的 `default_direct_message_events` + `try_commit_locked` | default lease 重新挂载是普通事件提交，不经 typed command receipt | 无阶段 receipt/query |
| snapshot/readback | `identity_context` 尾部 `Req::Context`；`handle_context` | 需要有效 worker token；旧 credential 失效时不可授权读取 | 缺 operation-key 查询路径 |

**关键事实**

- `CommandEnvelope` 强制 `command_id`、`operation_id`、`actor_binding_id`、`endpoint_generation` 和 `scope`：[proto.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/proto.rs:128)。`validate_for` 会把其 binding/generation/scope 与已注册 runtime 比较：[proto.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/proto.rs:187)。因此失效旧 binding 会让普通 command admission 失败，不能填造 `actor_binding_id`。
- Register 会拒绝不同 token，并可能以 `TOKEN_MISMATCH` 或 binding mismatch 失败：[part_06.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/mod_parts/part_06.rs:1050)、[part_10.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/mod_parts/part_10.rs:586)。
- `CommandStarted` 和 `CommandCompleted` 是事件类型：[state.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/state.rs:712)。`commit_command_locked` 先写 Start，再写业务事件，最后写 Completed；三个阶段分别 append+sync：[part_02.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/mod_parts/part_02.rs:652)、[part_02.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/mod_parts/part_02.rs:984)。
- reducer 对 `CommandStarted` 是 no-op；只有 `CommandCompleted` 把 receipt 投影到 `command_receipts`：[state_impl.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/state_impl.rs:634)。replay 遇到没有 Completed 的 Started command 会报 incomplete command，不能公开为 partial receipt：[part_12.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/mod_parts/part_12.rs:501)。
- command receipt 负责 command-id/operation-id 复用检查及冲突拒绝：[part_02.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/mod_parts/part_02.rs:566)、[global_state_impl_part2.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/global_state_impl_part2.rs:318)。

**Exact Gaps 与验收影响**

- operation key 在首次副作用前无法由外层调用者持有：CLI command envelope 在 `me()` 成功后生成，且使用进程号和 nonce，外层的 `IdentityContext` 没有 operation-id 字段：[main_context.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/main_context.rs:286)。影响 A3/A4/A5/A11 的丢响应、重复提交和 restart readback。
- 旧 credential 失效时 query 无法授权：`Req::Context` 需要 `worker_id + token`，并在 `handle_context` 先 verify：[proto.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/proto.rs:464)、[part_09.rs](/Volumes/Intel/playground/appsdk/collab-context-identity-peer-crud-20261008/collab/src/server/mod_parts/part_09.rs:1051)。影响 A4/A5/A11；A3 的拒绝路径可重复请求，但不能证明丢失响应对应的同一批准意图。
- route、credential file、lease 和 snapshot 是不同提交边界，没有统一外层 query 将它们的完成/未完成状态关联到一次恢复 operation。影响 A11 的 partial/unknown readback，以及 A4/A5 的重启后恢复。

**D2-B 需要的证据面**

D2-B 必须先冻结这些能力，不在此猜测 wire 字段：新增外层恢复 producer；稳定 operation identity 在副作用前产生；批准决策的 reducer/replay；对 nested Register receipt 的关联；route、credential、grant（仅适用替换时）、lease 的阶段记录；不依赖旧 credential 的 ownership-verified query；public projection；同意图复用与不同意图拒绝。现有 `CommandReceipt` 可作为内部候选基础，但不能从类型存在推断外层恢复已闭环。

**明确未知**

- 外层恢复应复用现有 host command journal，还是由 identity owner 扩展现有 `CommandReceipt` 语义，尚未由源码决定。
- host route journal 与 project runtime journal 的跨边界排序/关联合同尚未存在。
- query 的公开 CLI/MCP 形状、授权证明和错误分类尚未冻结。
- 当前 daemon、socket、installed binary 与候选源码等价性未在本节点验证。

**下一步**

`READY_FOR_CONTRACT`，可进入 D2-B 合同与图设计。D2-B 必须把上述 producer/reducer/replay/public projection/query 缺口转成精确契约；不得把 nested Register receipt 当成外层 receipt，不得填造 `actor_binding_id`，A6 继续保持 BLOCKED。