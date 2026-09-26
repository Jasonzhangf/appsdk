# Collaboration context state machine

One agent-facing entry: `collab context`. Its DAG is single-source
(`bootstrap_request`) and single-sink (`state_snapshot`). Failure terminals
are explicit and reported; retry uses a new attempt identity, not a graph
back-edge.

## Roles

- `agent`: calls `collab context`, reads the snapshot, acts on durable state.
  Never edits routes, mailbox, identity, or token.
- `cli context`: thin wrapper that owns the bootstrap sequence.
- `bootstrap`: owns project root resolution, `.agent-collab/` baseline, and
  the global daemon lifecycle.
- `identity`: owns the persisted local worker identity and token.
- `daemon register`: server-side handler owning registration, role
  derivation, and the default direct-message lease.
- `notification restore`: server-side handler re-arming the default
  direct-message lease from the registered transport.
- `master lookup`: server-side handler projecting the live master grant.

## State machine

```mermaid
stateDiagram-v2
    [*] --> 入口请求
    入口请求 --> 解析项目根 : 调用 collab context
    解析项目根 --> 已知路由 : route 命中
    解析项目根 --> 已知项目 : canonical-route 或 cwd baseline 命中
    解析项目根 --> 路径未识别 : 无 baseline 且无 git 根
    路径未识别 --> [*] : 报错 COLLAB_CONTEXT_UNRESOLVED
    已知路由 --> 检查基线
    已知项目 --> 检查基线
    检查基线 --> 检查守护 : baseline 已存在
    检查基线 --> 创建基线 : baseline 缺失且不在 playground
    创建基线 --> 检查守护 : 初始化完成
    检查基线 --> [*] : 拒绝在 playground 创建基线
    检查守护 --> 装载身份 : daemon 已存活
    检查守护 --> 启动守护 : daemon 未存活
    启动守护 --> 装载身份 : 守护启动完成
    装载身份 --> 身份命中 : 身份已存在
    装载身份 --> 创建身份 : 身份缺失
    创建身份 --> 注册对端
    身份命中 --> 校验令牌
    校验令牌 --> 注册对端 : 令牌匹配
    校验令牌 --> [*] : 报错 TOKEN_MISMATCH
    注册对端 --> 沿用注册 : 身份与路由匹配
    注册对端 --> 原地重建 : 线程或会话已变化
    注册对端 --> 新建注册 : 身份存在但路由不可用
    沿用注册 --> 恢复默认订阅
    原地重建 --> 恢复默认订阅
    新建注册 --> 恢复默认订阅
    恢复默认订阅 --> 默认订阅待命 : 非显式退订自动 rearm
    恢复默认订阅 --> 默认订阅已停 : 仅 owner 显式退订持久生效
    默认订阅待命 --> 查找主控
    默认订阅已停 --> 查找主控
    查找主控 --> 输出快照 : 主控已记录
    查找主控 --> 输出无主控快照 : 主控记录为空
    输出快照 --> [*]
    输出无主控快照 --> [*]
```

## Decision DAG

```mermaid
flowchart TD
    A[bootstrap_request] --> B{解析项目根}
    B -- 命中 --> C{检查基线}
    B -- 未命中 --> X1[报错: 路径未识别]
    C -- 已存在 --> D{检查守护}
    C -- 缺失 --> E[创建基线]
    E --> D
    C -- 在 playground --> X2[报错: 拒绝创建基线]
    D -- 存活 --> F{装载身份}
    D -- 未存活 --> G[启动守护]
    G --> F
    F -- 命中 --> H{校验令牌}
    F -- 缺失 --> I[创建身份]
    I --> J[注册对端]
    H -- 通过 --> J
    H -- 失败 --> X3[报错: TOKEN_MISMATCH]
    J --> K{路由是否匹配}
    K -- 匹配 --> L[沿用注册]
    K -- 线程或会话变化 --> M[原地重建]
    K -- 路由不可用 --> N[新建注册]
    L --> O[恢复默认订阅]
    M --> O
    N --> O
    O --> P{默认订阅状态}
    P -- 非显式退订 --> Q[默认订阅待命]
    P -- 显式退订 --> R[默认订阅已停]
    Q --> S[查找主控]
    R --> S
    S --> T{主控存在}
    T -- 是 --> U[输出快照]
    T -- 否 --> V[输出无主控快照]
    U --> Y[state_snapshot]
    V --> Y
```

## Terminals

- `路径未识别` -> `COLLAB_CONTEXT_UNRESOLVED`: preserve the error and report
  the registration problem; no daemon or identity is touched.
- `拒绝在 playground` -> explicit refusal; daemon, identity, and baseline
  stay untouched.
- `TOKEN_MISMATCH` -> the identity record and token are unchanged; re-derive
  a fresh identity through a real runtime anchor.
- `state_snapshot` -> the only sink; carries `bootstrap`, `identity`,
  `daemon`, `route`, `subscriptions`, `master`, `operations`, `peers`,
  `inbox`, `worktrees`, and `tasks`.

## Node owner mapping

| Semantic node | Handler | Source |
| --- | --- | --- |
| 解析项目根 | `resolve_context_root` | `collab/src/main.rs` |
| 创建基线 | `scope::init` | `collab/src/scope.rs` |
| 启动守护 | `client::ensure_server` | `collab/src/client.rs` |
| 装载身份 / 创建身份 | `identity::load_or_create` | `collab/src/identity.rs` |
| 校验令牌 | `verify` | `collab/src/server/mod.rs` |
| 注册对端 | `ensure_registration_with_outcome` | `collab/src/main.rs` |
| 沿用 / 原地重建 / 新建 | `Register` handler | `collab/src/server/state.rs` |
| 恢复默认订阅 | `default_direct_message_events` | `collab/src/server/mod.rs` |
| 查找主控 | `current_master_worker_id` / `current_master_grant` | `collab/src/server/mod.rs` |
| 输出快照 | `handle_context` | `collab/src/server/mod.rs` |

## Agent-facing rule

Call `collab context` once on bootstrap, and again only on thread/session
change, daemon restart, identity mismatch, or route loss. Every other action
reads from the resulting `state_snapshot`. `collab init`, `collab whoami`,
`collab worker recover`, `collab route resolve`, `collab down`/`up`, and any
direct transport call are operator-facing diagnostics, not part of the agent
flow.
