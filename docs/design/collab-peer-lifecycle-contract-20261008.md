# Collab Peer Lifecycle Contract: Read / Update / Close

Status: design candidate for independent review

This contract defines the peer lifecycle surface for:

- `Read`: an authoritative, side-effect-free view of peer lifecycle state.
- `Update`: a peer-owned mutation of mutable lifecycle intent.
- `Close`: a fenced lifecycle operation that must stop or retire the host-side
  runtime and the Collab control-plane responsibility set without inventing a
  successful terminal state.

`Create` is deliberately out of scope and remains blocked. This document does
not add a Create command, API, graph node, or implementation permission.

## 1. Scope and Authority

### 1.1 In scope

- Peer identity and selected transport.
- Peer-owned task responsibility.
- Runtime binding and current-thread route retirement.
- Host thread stop/archive where the selected adapter supports it.
- Notification and keepalive responsibility only where it is already part of
  peer close.
- Durable operation receipts, readback, partial results, and unknown results.

### 1.2 Out of scope

- Creating a peer, a host thread, a managed subagent, or a new identity.
- Replacing or promoting master authority.
- Changing a worker token, binding generation, scope, or route ownership.
- Deleting task history, mailbox history, or worktree history.
- Editing the Collab journal or a host route journal directly.
- Claiming that `WorkerClosed` alone equals a stopped host process.

### 1.3 Owner map

The following table distinguishes the current source owner from the proposed
contract. A proposal is not an implemented capability.

| Concern | Current owner | Current proof | Contract status |
| --- | --- | --- | --- |
| Peer request entry | `Req` in `collab/src/proto.rs:357` | `WorkerClose` (`:637`), `Context` (`:464`), `WorkerStatus` (`:635`), `TaskUpdate` (`:519`) variants | Existing surface |
| Close admission and responsibility check | `handle_worker_close` in `collab/src/server/mod_parts/part_07.rs:1317` | Master verification, self-close rejection, unfinished-owned-task rejection | Existing implementation, incomplete terminal semantics |
| Durable close record | `WorkerClosed` reducer in `collab/src/server/state_impl.rs:274` | Stores `WorkerCloseReceipt`, removes worker and keepalive | Existing implementation |
| Task update | `handle_task_update` / `task_update_locked` in `part_08.rs:1099` | Owner/token, transition table, durable `TaskUpdated` event | Existing implementation |
| Task cleanup | `handle_task_close`, `handle_task_finalize_cleanup` in `part_09.rs:767` | Cleanup receipt and idempotent finalization | Existing implementation; separate from peer close |
| Runtime binding / route retirement | `retire_runtime_binding_after_route_failure` in `part_06.rs:889`; `GlobalCurrentThreadRouteRetired` reducer in `state_impl.rs:765` | Durable route retirement exists for registration and launch-failure paths | Reusable foundation; not coupled to `WorkerClose` |
| Host thread archive | `archive_thread` in `collab/src/adapters/codex_app_server_production_part1.rs:918` | App Server `thread/archive` call exists | Capability exists; unused by `handle_worker_close` |
| Managed subagent close | `subagent.rs` action handler around `Action::Close` | Record-only close for tmux; archive/cleanup paths exist for managed launch failure | Separate lifecycle; must not create a second peer-close owner |
| Public read projection | `handle_context` in `part_09.rs:1051`, `worker_status_summary_with_maps` in `part_10.rs:288` | Context/status projections exist | Existing read surface; query semantics not unified |

## 2. Read Contract

### 2.1 Read is authoritative and side-effect-free

Read means reading the daemon’s committed lifecycle projection. It must not:

- register, recover, or mutate identity;
- write a route, binding, grant, task, message, keepalive, or close receipt;
- probe for the purpose of silently repairing state;
- treat a missing projection as proof that a peer is gone;
- treat an unknown host probe as `Closed` or `Missing`.

`Read` may perform only the minimum non-mutating observation needed to classify
the known projection. Host liveness observations must remain observations, not
implicit repair actions.

Current source conflict: `handle_context` at `part_09.rs:1051` restores the
default direct-message lease before returning its snapshot. That makes the
current `Req::Context` bootstrap path a read-plus-rearm operation, not a pure
lifecycle Read. The implementation must either expose a pure lifecycle query
projection or return the bootstrap commit as an explicit separate stage. A
snapshot that silently performs the rearm must not be used as evidence that
Read is side-effect-free.

### 2.2 Scope and authorization

The caller supplies the canonical project scope and the exact peer target where
the implementation supports one. The daemon validates:

- project root and project scope;
- app scope;
- caller identity and token where the existing request requires them;
- target binding and generation for any target that is not the caller;
- whether the caller is ordinary peer, managed peer, or current master.

The registered project identity is its canonical Git `main` root (or the
canonical project root when the project is not a Git worktree). Master and
peer registrations in one project use the same `(project_scope_id,
app_scope_id)` route. Their execution worktrees may be different directories,
including worktrees outside the main checkout; a worktree is an execution
location and cannot create a second project identity. Resolve execution cwd
through the existing registered-route owner before comparing it with the
target route. A shared directory ancestor or repository name alone does not
establish project membership.

The response must not expose managed assignment details to an ordinary peer.
It may expose only the ordinary public projection of another registered peer.

### 2.3 Required peer states

Read must distinguish these states:

| State | Meaning | Required evidence |
| --- | --- | --- |
| `Missing` | The exact durable target or its host identity is definitively absent. | A definitive absence result from the exact target binding or a committed missing record. |
| `Unknown` | The target may exist; the daemon cannot prove present, missing, or closed. | An explicit unknown probe result, unavailable transport, timeout, or unreadable evidence. |
| `Closed` | A durable close terminal exists for the exact target generation. | `WorkerCloseReceipt` plus the exact target binding/generation. |
| `CleanupOpen` | The peer close is recorded but a required control-plane retirement or cleanup obligation remains. | Per-stage receipt showing at least one unresolved retirement/cleanup stage. |

A lost response, empty list, token failure, or stale local credential must not
be reclassified as `Missing` or `Closed`.

### 2.4 Ordinary peer versus managed peer

- An ordinary peer reads its own public identity, transport, task, mailbox,
  subscription, and lifecycle projection.
- An ordinary peer may read another ordinary peer only through the public
  worker projection and never receives managed-child assignment or parent
  internals.
- A managed peer reads its own identity and its parent/assignment projection
  only when the managed relationship is already committed.
- A parent/master may read the managed relationship it owns, but the read
  remains a projection of committed state.
- A missing managed child cannot be inferred from a missing ordinary peer
  entry; it must be resolved against the managed subagent record and the exact
  host thread.

### 2.5 Read result matrix

Every Read result has a stable class and a source:

```text
ok: authoritative projection
refused: authorization or scope failure
missing: exact durable target is definitively absent
unknown: evidence is unavailable or ambiguous
closed: exact durable close terminal
cleanup-open: close terminal plus unresolved retirement stage
```

Read does not return `partial` or `cancelled`; those belong to mutating
operations. A query without a durable operation identity must not fabricate
one.

## 3. Update Contract

### 3.1 Initial allowed field

The first Update slice allows only the mutable `cwd` execution context. This is
a deliberate restriction, not a general field editor. The following fields are
frozen for Update:

- identity, worker id, token, runtime id;
- project/app scope;
- transport kind, endpoint, namespace, session, thread;
- binding id and endpoint generation;
- route ownership;
- master grant and authority;
- parent/kind;
- task ownership, mailbox ownership, and worktree ownership.

An Update request attempting any frozen field must be rejected before a
durable mutation. A successful settings acknowledgement is not proof that a
later target execution uses the new `cwd`; a later real execution must prove
the new working directory before the projection is treated as effective.

The allowed `cwd` is an existing, canonicalizable directory that the existing
`canonical_route_for_identity` route owner resolves to the target's exact
registered `(project_scope_id, app_scope_id)`. This resolver maps linked Git
worktrees to their registered main route. Compare the resolved route with the
target's committed route; do not compare the raw canonical cwd with the
registration root. Keep the registration root unchanged, and never register
an execution worktree as another project. Reject a missing or ambiguous route,
a different main or app scope, or an unregistered worktree before any durable
or host mutation. A scope mismatch is a refusal, never an implicit
re-registration or route migration.

### 3.2 Unique state owner and durable intent

The daemon lifecycle owner owns the Update admission and the target state
record. A caller may supply intent, but it may not write the record directly.
The Update operation must:

1. resolve the exact target and current binding generation;
2. validate caller ownership or explicit authority;
3. save the intended change before any host-side effect;
4. commit the durable intent;
5. perform or schedule the host-side change;
6. read back the exact committed result;
7. return `complete`, `partial`, `unknown`, `refused`, or `cancelled`.

The current source proves owner/token checking and `TaskUpdated` persistence
for task Update, but it does not prove a peer `cwd` Update command, a durable
operation record, or a later effective-cwd readback. Those are design gaps.

The source search over `Req`, CLI commands, and registered handlers found no
peer `Req::Update`/`Req::UpdateCwd` variant. `Req::TaskUpdate` updates task
status/next-step only. Worker `cwd` is read from registration and is used by
`GlobalState::canonical_project_scope`/route checks; changing it is a
scope-affecting operation, not an ordinary field edit. Any future Update
implementation must route through the unique lifecycle/identity owner and
must not mutate `WorkerRec.cwd` directly.

### 3.3 Idempotency and conflict

The operation identity must be durable before the change. Repeating the same
operation identity and canonical intent returns the original result. The same
operation identity with a different intent is rejected. A new operation may
retry only after the prior result is `refused`, `complete`, or explicitly
proven safe to retry. A `partial` or `unknown` result requires an operation-key
readback; it does not authorize blind re-execution.

The existing `CommandReceipt` machinery may be reused as an internal basis only
after the outer lifecycle operation identity is frozen. This contract does not
invent common receipt fields that D2 has not frozen.

### 3.4 Update responsibility fence

The Update admission must fence:

- project/app scope;
- target identity and binding generation;
- the old and new `cwd`;
- caller ownership;
- current task/mailbox/worktree responsibility;
- the operation identity.

Update refuses while the target owns an unfinished task, has a live mailbox or
notification obligation whose handling depends on the current working
directory, or owns an active worktree assignment that would be stranded by the
change. The refusal names the conflicting task/obligation/assignment IDs and
has no host or durable target mutation. Once those responsibilities are
resolved, a same-scope `cwd` change may proceed; ownership and task/mailbox/
worktree records remain unchanged. This first slice does not move or delete a
worktree.

If the host effect succeeds but the daemon cannot commit the control result,
the result is `partial` or `unknown`, never `complete`. The target must not be
rebuilt, replaced, or re-registered to make the response green.

## 4. Close Contract

### 4.1 Close target identity

Close must bind all of the following before admission:

- exact target peer id;
- exact project/app scope;
- target binding id and endpoint generation;
- selected transport kind and endpoint;
- caller identity and authority;
- durable operation identity.

No Close may use a display name, latest thread, a wildcard target, or a
guessed binding.

### 4.2 Responsibility check and admission fence

The responsibility check and the close intent must share one admission fence.
The fence is checked at close admission and rechecked after every host-side
call that could change the target. A check performed only at the outer CLI or
only at the first handler is insufficient.

The lifecycle owner persists a `closing` fence for the exact target peer,
project/app scope, binding generation, and close operation before any host
call. The fence is durable state, not an in-memory mutex: restart/replay must
restore it before accepting writes. While it exists, every writer that can add
or change target responsibility must either reject with `target_closing` or
participate in the same serialized admission transaction. This includes:

- task registration/status and scheduler assignment (`handle_task_register_with_next` in
  `collab/src/server/mod_parts/part_08.rs`,
  `handle_scheduler_dispatch`, and the reducer commit path for `TaskCreated`);
- managed-child assignment (`SubagentUpdated` through the scheduler and
  subagent action owner);
- worker registration and binding/route publication
  (`handle_register_with_app_scope`, `handle_register_with_app_scope_unfinalized`,
  and `GlobalCurrentThreadRouteSet` commits);
- notification subscription create/update/cancel
  (`handle_notification_subscribe` and `handle_notification_unsubscribe`),
  plus notification delivery/consumption when it creates a new obligation.

Read-only status, mailbox/history reads, and consumption of already-durable
messages may continue if they cannot create a new owner or responsibility.
Every accepted responsibility writer rechecks the exact target generation and
fence in the same durable commit that writes its event; checking before an
unlocked host call is insufficient. Close first writes the fence and its
operation identity atomically with the responsibility snapshot. No global
state lock is held across a host RPC.

The fence remains after host success until exact host-stop evidence, worker
retirement, binding/route retirement, and applicable lease/subscription
retirement have been read back and the close terminal is durable. On host
refusal or a proven no-effect cancellation, clear the fence only after a
durable `refused`/`cancelled` terminal and a readback confirming the target is
still registered at the same generation. On partial or unknown host/control
results, retain the fence across restart and expose `cleanup-open`/`unknown`
through operation query; do not reopen writers or repeat the host action
automatically. An operator repair can clear it only after query proves either
the target remains active and all prior stages had no effect, or lifecycle
retirement is complete. The repair is an explicit new durable operation.

At minimum, Close classifies:

| Responsibility | Required decision |
| --- | --- |
| Unfinished task | Reject close or require an explicit task lifecycle resolution first. |
| Unread notification | Preserve, retire, or require explicit disposition; never silently claim delivery. |
| Running host instance | Stop only through the exact selected host action. |
| Registration | Retire only the exact target binding/generation. |
| Binding/route | Retire through the binding/route owner and preserve a tombstone or explicit missing state. |
| Applicable lease/subscription | Retire through its owner where the contract applies. |
| Grant | Do not change master authority as a side effect of ordinary Close. |
| Task/mailbox/history/worktree/receipt | Preserve; Close does not delete history. |

The source currently proves unfinished-task and self-close rejection in
`handle_worker_close`. It does not prove a complete notification, lease,
subscription, binding, route, or host-instance fence. Those are implementation
gaps, not waived requirements.

### 4.3 Host stop and control-plane retirement are separate owners

Close has two terminal obligations:

1. **Host terminal**: stop the exact selected host instance or archive its
   thread through the real adapter. The App Server adapter has
   `thread/archive`; tmux has no proven peer-pane termination here.
2. **Control-plane terminal**: retire the exact registration, binding, route,
   and applicable lease/subscription through their owners.

- The unique host-stop owner is the selected transport's real host adapter. The
  daemon lifecycle owner may request the action but may not impersonate the
  host adapter or substitute a journal record for the host effect.
- Current source proves an App Server `thread/archive` call. It does not prove
  a generic process-stop, turn-termination, or tmux pane-termination capability
  for an ordinary worker. If the selected adapter cannot stop or terminally
  archive the exact target, Close cannot return `complete`; it must return
  `partial`, `unknown`, or `cleanup-open`.
- The unique control-plane retirement owners remain the original binding,
  route, grant, and applicable lease/subscription owners. The daemon lifecycle
  owner coordinates their receipts but does not write their records directly.

`WorkerClosed` is a durable Collab close record, not a substitute for a host
stop or route retirement. No implementation may use a PID kill, a broad
process kill, or an unowned external command as an implicit host-stop owner.

The host RPC must run outside the global state lock. After it returns, the
handler must recheck operation identity, binding generation, and target
ownership before committing the control-plane terminal.

### 4.4 Close result model

The result is one of:

| Result | Meaning |
| --- | --- |
| `complete` | Host terminal and all required control-plane retirements are durably read back for the exact operation and generation. |
| `refused` | A responsibility or authorization rule rejects the request before the mutation. |
| `partial` | At least one required stage committed, but another stage is known incomplete. |
| `unknown` | A host or journal boundary was crossed without a provable terminal outcome. |
| `cancelled` | Cancellation is true only when the contract supplies a real ownership/cancellation boundary and the exact target is known. |
| `cleanup-open` | A durable close record exists but a required retirement/cleanup obligation remains. |

An archive acknowledgement alone is not `complete`. A `WorkerClosed` record
alone is not `complete`. A timeout, decode failure, lost response, or unknown
host result is never converted into `Closed`.

### 4.5 Partial, unknown, retry, and readback

If the host stop is definite but control-plane retirement fails, the result is
`partial` or `cleanup-open`; the target is not rebuilt. If the host result is
unknown, the result is `unknown` and the operation remains queryable by its
durable operation identity. Retry must:

- reuse the same operation identity and canonical intent;
- query first when the prior result is `partial` or `unknown`;
- never repeat `thread/archive` or another host effect blindly;
- never mark a weaker old receipt as complete.

### 4.6 Historical compatibility

Older record-only close receipts may be read for audit and idempotent replay,
but they must be labeled by their actual terminal class. They do not
automatically upgrade to a full lifecycle `Closed`. A record-only receipt may
be returned only as:

```text
close_outcome: closed_record_only
runtime_archive: unknown/not-attempted
control_plane_retirement: unknown/not-attempted
```

If a later operation retires the remaining stages, it must issue a new exact
operation identity and bind the old receipt as historical input, not rewrite
it.

### 4.7 Missing boundary and uncertainty

A definitive `Missing` target may be retired without a live thread snapshot
only when the exact address is proven missing and the responsibility set is
resolved. `Unknown` never becomes `Missing`. A cold thread is not a missing
thread. A stale route is not proof that the peer process stopped.

## 5. Create Guard (A6 remains blocked)

Create is limited to a blocked guard, not a hidden implementation path.

The guard remains blocked until either:

1. a stable intended request is associated with the exact host thread before
   any side effect, survives response loss and restart, and is publicly
   queryable; or
2. an exclusive ownership handle exists before send and can cancel exactly
   that intent with a durable cleanup receipt.

These do not satisfy the guard:

- schema-level idempotency;
- JSON-RPC request id;
- an empty project list;
- stopping the endpoint process;
- a post-response thread id;
- a nonce, latest thread, or guessed association.

No graph node, command, API, or runtime path in this design may create a peer.

## 6. State Machine

### 6.1 Read

```text
Idle
  -> Requested
  -> Classified
  -> Missing | Unknown | Closed | CleanupOpen | ProjectionReady
```

Transitions:

- `Requested + scope/authorization valid -> Classified`
- `Classified + exact absence evidence -> Missing`
- `Classified + unavailable/ambiguous evidence -> Unknown`
- `Classified + durable close terminal + no open cleanup -> Closed`
- `Classified + durable close terminal + open cleanup -> CleanupOpen`
- `Classified + committed projection -> ProjectionReady`

No Read transition writes mutation state.

### 6.2 Update

```text
Idle
  -> Requested
  -> Admitted
  -> IntentPersisted
  -> EffectApplied | Refused | Unknown
  -> ReadbackComplete | Partial | Unknown | Cancelled
```

`Refused` terminates before intent persistence. `IntentPersisted` is the
durable fence against duplicate intent. A retry starts a new attempt identity
but reuses the same operation identity.

### 6.3 Close

```text
Idle
  -> Requested
  -> Admitted
  -> ResponsibilityChecked
  -> HostStopping
  -> HostStopped | HostUnknown | Cancelled
  -> ControlRetiring
  -> Complete | Partial | CleanupOpen | Unknown
```

Invalid transitions include:

- `Requested -> Complete` without responsibility check;
- `HostUnknown -> Complete`;
- `HostStopped -> Complete` without the required control-plane readback;
- `Closed record -> HostStopped` without an exact new operation;
- `CleanupOpen -> Complete` while a required retirement stage is unresolved.

## 7. DAG Contract

The graph file is
`docs/dagpipe/collab-peer-lifecycle.graph.json`.

It is one SESE decision graph from a lifecycle request to one result. The
semantic path is:

```text
接收生命周期请求
  -> 解析目标、范围与操作意图
  -> 校验身份、授权与责任边界
  -> 分类为 Read / Update / Close / Create 阻断守卫
  -> 执行读、更新、关闭或创建阻断分支
  -> 读回并分类主机与控制面阶段结果
  -> 输出唯一生命周期结果
```

The graph uses guards and shared result classification. It must not imply an
automatic retry, a fallback transport, a second state owner, or a successful
Create path.

### 7.1 Operator mapping

| Semantic node | Proposed operator | Current source mapping |
| --- | --- | --- |
| 接收生命周期请求 | `appsdk.collab_peer_lifecycle.receive_request` | `Req` dispatch in `collab/src/server/mod_parts/part_04.rs` and CLI `main.rs` |
| 解析目标、范围与操作意图 | `appsdk.collab_peer_lifecycle.parse_intent` | `Req::Context`, `Req::WorkerStatus`, `Req::TaskUpdate`, `Req::WorkerClose` in `proto.rs` |
| 校验身份、授权与责任边界 | `appsdk.collab_peer_lifecycle.admit_request` | `verify`, master verification, responsibility checks in `part_07.rs` and task handlers |
| 执行读、更新、关闭或创建阻断分支 | `appsdk.collab_peer_lifecycle.execute_lifecycle_branch` | Read / Update / Close / Create blocked-guard branch dispatcher; current source is split across `handle_context`, `handle_task_update`, `handle_worker_close`, and no Create path |
| 读回并分类生命周期结果 | `appsdk.collab_peer_lifecycle.readback` | `WorkerCloseReceipt`, `CleanupReceipt`, route/binding projections; unified operation readback and result classification are gaps |
| 输出唯一生命周期结果 | `appsdk.collab_peer_lifecycle.emit_result` | `Resp` and existing JSON projections; typed lifecycle result is a gap |

## 8. Compatibility Dependency on D2

This contract depends on D2-A and the D2-B interface freeze for:

- the outer operation identity;
- receipt producer/reducer/replay;
- public query authorization and shape;
- partial/unknown representation;
- exact scope/target/action normalization.

Until D2-B is independently reviewed and frozen, those common receipt and query
fields are marked **interface pending freeze**. This contract must not invent
wire fields or claim that a source type exists merely because a similar type
exists.

## 9. Acceptance Matrix

| Case | Input | Expected result | Forbidden result |
| --- | --- | --- | --- |
| R1 ordinary self-read | Exact ordinary peer scope/token | Authoritative projection | Any mutation |
| R2 unknown probe | Transport unavailable/timeout | `Unknown` | `Missing` or `Closed` |
| R3 closed read | Exact generation plus close receipt | `Closed` or `CleanupOpen` | `ProjectionReady` with false completion |
| R4 managed read | Parent reads owned child | Managed relationship projection | Ordinary peer leaking managed internals |
| U1 owner cwd update | Exact owner and new cwd | Durable intent plus later effective-cwd proof | ACK-only success |
| U2 duplicate same intent | Same operation id/intent | Same original result | Second effect |
| U3 duplicate conflict | Same operation id/different intent | Refusal | Silent overwrite |
| U4 identity field update | Attempt to change token/scope/binding | Refusal before mutation | Identity drift |
| U5 true cancellation boundary | A real owned cancellation handle and exact target | `cancelled` with preserved target facts | `cancelled` for timeout or lost response |
| U6 same-main cwd | Existing directory resolves through the registered route owner to the exact same canonical main and app scope; master and peer may use different worktrees; no conflicting responsibility | Update proceeds and later execution proves the new cwd; scope, binding, route, and ownership are unchanged | New registration root, route change, or ACK-only completion |
| U7 cross-scope cwd | Route resolution identifies a different canonical main or app scope, or cannot prove a registered route | Refusal before durable or host mutation | Silent scope migration or replacement binding |
| U8 cwd responsibility conflict | Exact target owns an unfinished task, live cwd-dependent obligation, or active worktree assignment | Refusal naming each conflicting ID with no side effect | Partial update, responsibility transfer, or silent worktree move |
| C1 unfinished task | Close target owns unfinished task | Refusal naming task ids | `complete` |
| C2 host stopped, control failed | Exact host stop succeeds; route retire fails | `partial`/`cleanup-open`/`unknown` | Rebuild target or `complete` |
| C3 host unknown | Timeout after host request | `unknown` with query path | Blind `thread/archive` retry |
| C4 record-only close | Historical receipt only | `closed_record_only` with unknown stages | Full `Closed` claim |
| C5 missing target | Exact address definitively missing and responsibilities resolved | Retirement with explicit missing evidence | Snapshot fabrication |
| C6 current master self-close | Master targets itself | Refusal | Headless project |
| C7 no host-stop capability | Selected adapter lacks terminal stop/archive | `partial`/`unknown`/`cleanup-open` | `complete` or direct PID kill |
| C8 writer races close | A task/assignment/registration/subscription writer runs after durable close admission fence and before host response | Writer refuses `target_closing`; no new responsibility is committed; Close keeps the fence through terminal readback | New responsibility committed while the host is being stopped |
| C9 restart during close | Daemon restarts after fence persistence and before host/control terminal | Replay restores the fence; query exposes the same operation and phase; no writer admission or automatic repeated host action | Fence loss, duplicate host action, or target shown as fully open |
| Create guard | Any Create request | Blocked dependency result | New command/API/graph path |

## 10. Static Verification for This Design

The design worker must run only:

```sh
jq empty docs/dagpipe/collab-peer-lifecycle.graph.json
dagpipe graph validate docs/dagpipe/collab-peer-lifecycle.graph.json
git diff --check -- docs/design/collab-peer-lifecycle-contract-20261008.md docs/dagpipe/collab-peer-lifecycle.graph.json
sha256sum docs/design/collab-peer-lifecycle-contract-20261008.md docs/dagpipe/collab-peer-lifecycle.graph.json
```

These checks prove JSON syntax, static SESE shape, whitespace validity, and
artifact identity only. They do not prove implementation behavior, host
adapter behavior, or Create readiness.

## 11. Known Gaps and Next Admission Conditions

### 11.1 Known gaps

- No unified peer lifecycle operation identity or public query is implemented.
- `Req::WorkerStatus` has no caller token in the current wire shape; it is a
  public projection, not an authoritative authorization-bearing lifecycle
  query.
- No peer `cwd` Update request or handler exists; current Update source support
  is task status/next-step only.
- `handle_worker_close` does not archive or stop the host thread.
- `handle_worker_close` does not retire runtime binding or route entries.
- No shared responsibility fence exists for every relevant writer.
- Notification and subscription retirement are not fully modeled in the close
  handler.
- Peer `cwd` Update has no proven public command or effective-cwd readback.
- D2-B receipt/query fields remain unfrozen.

### 11.2 Next admission condition

This candidate may proceed only to independent design review. Implementation
admission requires:

1. an independent review PASS for this contract and graph;
2. frozen D2 receipt/query interfaces;
3. precise implementation paths for the lifecycle owner, host adapter, binding
   owner, route owner, task owner, and notification owner;
4. explicit tests for every acceptance case above, including negative and
   unknown-result cases;
5. a separate Create admission after the A6 evidence threshold is met.
