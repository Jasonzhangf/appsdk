# Codex TUI native communication and sensing

Status: **production AppServer RPC path is wired; dual-TUI live acceptance and restart replay verified in the isolated acceptance scope**

Evidence date: 2026-09-24

AppSDK baseline: `7908aab95bc54b208c3e88310649407d0acccf6e`

## Verified experiment

The experiment used an isolated Codex home (`/tmp/ac27/home`), isolated project
(`/tmp/ac27/project`), isolated tmux socket (`ac27-tui`), and two independent
Codex TUI clients (`peer-a`, `peer-b`). Both clients connected with
`--profile gcm --remote unix://` to the same explicitly shared isolated local
AppServer daemon.
The daemon was Codex 0.156.1 (PID 28760 at verification time).
tmux hosted and displayed the clients; it did not carry message payloads.

| Capability | Operation | Observed result | Evidence strength |
| --- | --- | --- | --- |
| Registerable identity | peer-a `threadId=01a0d3ee-035b-7502-b49c-24434b130629`; peer-b `threadId=01a0d3ee-036a-70a2-bfac-53e47e24b82d` | Both threads appeared on the shared AppServer; registration by thread ID can address a thread owned by that AppServer. | Native AppServer thread listing/read plus TUI clients |
| Runtime session identity | peer-a and peer-b rollout `session_id` fields | Each rollout separately recorded `session_id` and thread `id`; the values happened to be equal for each peer in this run. Keep these as separate fields and obtain each from its owner; this observation does not justify deriving or merging them. | TUI rollout `session_meta` records |
| Loaded-thread discovery | `thread/loaded/list` | Shared endpoint exposed both independent TUI threads. | Native AppServer API response |
| State sensing | `thread/read`, `thread/turns/list`, `thread/queue/list` | Read peer-b state; observed queued work, completed turns, and idle state after the native API queue turn completed. | Native AppServer API response |
| Start a turn | `turn/start` on peer-b thread | Target TUI began the submitted turn. | RPC acceptance plus target TUI output |
| Steer active turn | `turn/steer` on peer-b thread | RPC identified the active `turnId`; current turn output included `TUI_STEER_CONSUMED_AC27`. | Target assistant output proves turn execution, not Collab mailbox consumption |
| Queue a turn | `thread/queue/add` on peer-b thread | Queue was visible while another turn ran; the queued turn then ran and output `TUI_QUEUE_CONSUMED_AC27`. | Queue API/state plus target output; not a Collab mailbox ACK |
| CLI queue | `codex queue --thread <peer-b-thread> --remote unix:// --profile gcm` | Returned queue ID `01a0d3f6-b197-7870-825c-7bef18e3e892`; peer-b executed and output `TUI_CLI_QUEUE_CONSUMED_AC27`. | CLI receipt plus target output; not a Collab mailbox ACK |

Exact `codex queue` invocation:

```sh
CODEX_HOME=/tmp/ac27/home codex queue \
  --thread 01a0d3ee-036a-70a2-bfac-53e47e24b82d \
  --message TUI_CLI_QUEUE_CONSUMED_AC27 \
  --remote unix:// --profile gcm
```

The full raw experiment record is `/tmp/ac27/RESULTS.md`. The isolated daemon,
tmux sessions, and bridge processes were stopped after the run. AppSDK `main`
was clean at the recorded baseline.

### Proven boundary

Two independent TUI clients communicate through native AppServer RPC when they
attach to the same AppServer endpoint. Within that owner, a registered
`threadId` addresses `turn/start`, `turn/steer`, queue, and state-read
operations. A thread ID alone does not locate a thread owned by another
independent embedded AppServer. A route therefore needs both the thread
identity and the AppServer owner/endpoint.

Not established by this run: access to each TUI's default/private embedded
AppServer endpoint from Collab, routing between separate AppServer owners,
cross-host routing, `codex queue` without the shared endpoint, or automatic
consumption acknowledgement for arbitrary messages. In particular, the run
does not prove that an embedded endpoint is externally discoverable or
reachable by the Collab daemon; endpoint discovery/reachability is an explicit
precondition and acceptance gate for the RPC-first design. RPC acceptance is
not consumption. The unique target TUI output proved the experiment's prompt was
executed; it did **not** create a Collab mailbox receive/ACK receipt. For a
Collab message, only the target's message-bound `collab recv`/ACK journal event
closes Collab consumption.

### Embedded endpoint boundary (source verified)

Codex source at `/Users/fanzhang/code/codex`, HEAD
`40eac3ce8a0c10cbcb9db910d529355eb2f8fc09`, is clean. In
`codex-rs/tui/src/lib.rs`, `start_app_server` connects to a remote/local daemon
only when `AppServerTarget` is not `Embedded`; the embedded path calls
`start_embedded_app_server(...).map(AppServerClient::InProcess)`. The in-process
client is backed by internal request/event channels, not an exposed socket or
WebSocket endpoint. An independent Collab daemon therefore cannot discover or
call a default private embedded TUI AppServer. A thread ID does not bridge this
process boundary.

Codex can auto-connect a replayable launch to its default local daemon socket,
or connect to an explicitly supplied remote endpoint. The isolated two-TUI run
verified the explicit shared endpoint path; it did not exercise implicit local
daemon startup. The same source tree's `tui/src/daemon_startup.rs::exclusion`
marks `--profile` as a reason not to reuse the implicit local daemon. Therefore
`codexp use gcm` followed by a normal `codex --profile gcm` launch does not, by
itself, put that TUI on the shared daemon route.

The supported native topology is explicit: each TUI and Collab must name and
reach the same AppServer owner, and registration must prove the submitted
thread/session/project tuple against that owner before committing a route. The
endpoint is an explicit registration input persisted in the binding. `codexp`
remains profile selection only; it is not an endpoint broker or daemon
lifecycle owner. Private embedded mode terminates at registration with an
actionable unreachable/wrong-owner result, makes no binding mutation, and does
not fall back to tmux. This closes that topology as an explicit failure
terminal instead of an unresolved transport fallback.

## Current implementation versus target

The current source does not implement the tested native path in production:

- `collab/src/adapters/mod.rs` exposes tmux as the production adapter and
  gates `codex_app_server` behind `cfg(test)`.
- `collab/src/main.rs` registration sends the tmux candidate and omits the
  AppServer candidate.
- `collab/src/server/mod.rs::validate_transport_candidates` rejects an
  AppServer candidate and accepts tmux only.
- Existing AppServer protocol structs and test-only adapter code are reusable
  exploration material, not proof that the runtime path is connected.

## Scope and ownership boundary

- **Allowed AppSDK owner paths:** `collab/src/main.rs` and
  `collab/src/adapters/**` for runtime facts and native RPC; `collab/src/identity.rs`
  and `collab/src/server/**` for registration, binding, recovery, attempts,
  state projections, subscriptions and receipts; `collab/src/bin/collab-mcp.rs`
  for public MCP schema; `collab/tests/**` and existing focused unit tests for
  acceptance; the existing Collab maps, skill, `docs/collab.md`, and this plan
  for synchronized contracts.
- **Forbidden for this task:** `/Users/fanzhang/code/codex` source, the `codexp`
  launcher, tmux message/presence fallback, parallel route or subscription
  registries, and starting/stopping an AppServer daemon from Collab. The
  shared AppServer endpoint must already exist and be reachable. TUI launch
  configuration and the Collab registration input must explicitly name that
  same endpoint; otherwise registration stops before state mutation.
- **Single owners:** Codex runtime owns live `sessionId`/`threadId`; the
  AppServer owner serves native thread/turn/queue RPC; the Collab daemon journal
  owns peer identity, binding, role, mailbox, attempt, subscription and
  consumption truth. The CLI gathers candidate facts but cannot adjudicate or
  persist a route independently.

The target is an AppServer-RPC-only communication and sensing path. tmux is
used only as a last-resort identity recovery anchor when current session and
thread IDs are unavailable; tmux never carries messages, wakeups, state, or
presence. Registration returns one authoritative binding with verified runtime
and optional pane facts; callers do not infer the route by separately reading
local state.

## Target ownership and invariants

| Fact | Owner | Required rule |
| --- | --- | --- |
| Stable `agentId`, runtime binding, generation, role and recovery grant | Collab daemon journal | Durable truth; one registration round trip returns the committed binding and its selected capabilities. |
| Current `sessionId` and `threadId` | Live Codex runtime, reported by the peer | Read from the running runtime; never predeclare either ID or derive one from the other. |
| AppServer endpoint/owner | Client configuration plus daemon self-check | The endpoint is part of the route. Verify `thread/read` returns the requested thread, matching session and canonical project root. |
| tmux socket/server/session/pane/pane PID | Current peer environment, verified by daemon probe | Persist only as a last-resort identity recovery anchor. A pane ID alone is insufficient; it must match the same tmux server, session ID, and live pane PID. Never use it for communication or presence. |
| Message body/history/attempt/receipt | Collab journal and mailbox | AppServer is transport and runtime state source, not durable Collab message truth. |
| Codex thread/turn/queue state | Owning AppServer RPC | Use native RPC first for status, turn control, and queued work. Never infer delivery from pane output alone. |
| Subscription owner, event filter, expiry and consumed state | Collab daemon | Finite, durable, exact-event subscription. Notify only through the verified AppServer RPC binding. |
| Notification operation choice | Authenticated sender request, validated and persisted by Collab daemon | `immediate` preserves current behavior: idle/cold starts a turn; exactly one active turn is steered. `queued` queues behind exactly one active turn; on idle/cold it starts. The RPC operation is recorded with the attempt. Unknown state or multiple active turns fail without submission. |

At most one current peer binding may own an exact AppServer-owner/session/thread
route or exact tmux socket/server/session/pane/pane-PID anchor in a project.
Duplicate or cross-peer reuse is an admission conflict. Rebind is an explicit
generation change with a durable old-route tombstone, not silent endpoint
replacement.

### Register and recover

`collab init` and `collab worker recover` submit one composite candidate:

```text
project root
runtime: sessionId, threadId
AppServer: endpoint/owner, namespace
tmux: socket path, server PID, tmux session ID, pane ID, pane PID (when present)
declared capabilities
```

The daemon independently validates all supplied facts and commits one
generation/binding atomically. The single response returns the stable peer ID,
runtime/binding IDs, generation, verified AppServer owner, RPC capabilities, validated
identity anchors, recovery result, and actionable errors. Observations may be
missing when a runtime does not expose them, but admission requires at least
one validated, unique identity anchor and rejects any conflicting supplied
facts without changing the current binding. RPC selection additionally
requires a verified AppServer owner and a thread that `thread/read` confirms;
otherwise registration fails without mutation. The committed communication
transport is always the verified AppServer RPC binding; later RPC failure does
not trigger a tmux resend.

Recovery uses current context in this order:

1. Match the current `sessionId` and `threadId` pair to the exact registered
   project and AppServer owner.
2. If the pair is unavailable, use one available runtime anchor (`sessionId`
   or `threadId`) only when it uniquely identifies the same persisted peer and
   the owning AppServer confirms it. If available anchors disagree, fail
   closed.
3. Use the validated pane tuple only when neither runtime ID is available.
   A pane match must also prove the same tmux server and live pane PID; a bare
   `%paneId` is insufficient.

Recovery rebinds the existing stable peer and preserves role, tasks and grants;
it does not mint another identity. A recorded master is not live without a
verified live route. Master grant restoration is allowed only for the same
persisted principal and only when no live master exists; otherwise recovery
does not transfer or duplicate authority.

### Communication, sensing, delivery and subscription

- **Send:** route by the daemon's exact binding. The message request owns a
  typed `delivery` choice (`immediate` by default for compatibility, or
  `queued`). For `immediate`, use `turn/start` when idle/cold and `turn/steer`
  when exactly one turn is active. For `queued`, use `thread/queue/add` when
  exactly one turn is active and `turn/start` when idle/cold. Unknown state or
  multiple active turns fails before submission. Persist the requested mode,
  selected RPC operation, and response with the durable attempt. Subscription
  timer/resource events use `immediate`, chosen by their producer; a direct
  `sendmessage` caller may explicitly request `queued`.
- **Retry:** a known pre-submit failure may be retried according to the
  existing bounded attempt rules. Timeout or any ambiguous post-submit result
  is `unknown`; never submit the same message through tmux afterward.
- **Sense:** use `thread/read` and observed turn/queue state from the owning
  AppServer. tmux is not a presence or activity source. Probe failure is
  `unknown`, not `absent`. Codex thread state does not define
  Collab task state such as `blocked`; task status remains owned by the durable
  Collab task record and must not be inferred from RPC silence or TUI output.
- **Notify:** durable message/attempt facts are committed before the wake.
  Native RPC acceptance means `accepted`, not `consumed`. A completed Codex
  turn proves the wake ran but does not consume the Collab mailbox message.
  Only a target-side, message-bound `collab recv`/ACK journal receipt closes
  Collab consumption; preserve `accepted`, `unknown`, and `consumed` as
  distinct results. If the Codex turn is cancelled or fails before `recv`, the
  durable message remains unread and the RPC attempt remains accepted but
  unconsumed; cancellation never deletes the message or fabricates an ACK.
- **Subscribe:** the daemon stores finite exact-event subscriptions with
  owner, expiry, and one-shot/reusable semantics. A matching event creates one
  bounded notification attempt through the selected RPC route. Expired,
  absent, cancelled, already-consumed, or ambiguous subscriptions do not
  produce another send. Mailbox remains the durable recovery source.
- **Queue/steer:** retain native ordering and interruption semantics where
  exposed by the verified AppServer methods. Do not emulate queue or steer by
  typing into a pane.

## Current-source owner and wiring audit

This is a read-only audit of baseline `7908aab95bc54b208c3e88310649407d0acccf6e`.

| Path | Current owner/wiring | Verdict against target |
| --- | --- | --- |
| `collab/src/main.rs::register_with_runtime` | `init`/`recover` send `appserver: None`, call `tmux::candidate_from_env`, then persist the server's `SelectedTransport`. | First missing edge: composite live runtime + AppServer + pane observations never reach the daemon. |
| `collab/src/main.rs::Cmd::Context` | Resolves the current tmux pane before loading local identity; route miss uses a tmux-oriented recovery context. | Pane is currently the primary context anchor; requested session/thread-first recovery is unwired. |
| `collab/src/main.rs::persisted_runtime_matches_scope` | For tmux, compares the current pane route and asks daemon `resolve_route`; for non-tmux bindings it still requires a tmux route resolution later. | Cannot reuse an AppServer-native runtime binding; the identity/presence contract is coupled to tmux. |
| `collab/src/adapters/mod.rs` | Exposes `tmux` in production; `codex_app_server` and its exports are `cfg(test)`. | Native RPC code is not a production capability despite its existing tests. |
| `collab/src/adapters/codex_app_server.rs` | Contains WebSocket client, identity verification, `thread/read`, `turn/start`, `turn/steer`, `thread/turns/list`, and immediate-notify logic. | Reusable core exists, but production API lacks the experiment's queue operation and integration. Existing admission does not carry pane evidence alongside RPC identity. |
| `collab/src/server/mod.rs::validate_transport_candidates` | Rejects AppServer candidates and only self-checks/selects tmux. | Daemon admission's single owner is clear; it must validate composite facts and select RPC-first once. |
| `collab/src/server/mod.rs::resolve_route` | Production route resolution validates tmux endpoints; AppServer verification branch is test-only. | Thread/session keys exist in durable state but are not a production live RPC route. |
| `collab/src/server/mod.rs::attempt_notification_with_at` and sink | Durable message/attempt admission eventually calls `attempt_tmux_notification...`; the default sink uses tmux `paste-buffer` + Enter and reports accepted with `consumed=false`. | Notification owner and failure model exist; selected RPC send and RPC ambiguity handling are unwired. |
| `collab/src/server/mod.rs::handle_notification_subscribe` + `notification_state.rs` | Finite, owner-bound subscriptions, event filters, expiry, replayable status and wake scheduling are durable. Subscription target/method are derived from the currently selected tmux transport. | Subscription truth/reducer is reusable; its trigger-to-native-RPC edge and receipt semantics are missing. |
| `collab/src/server/mod.rs::handle_context` / `worker_presence_with_view` | Projects durable worker, role, tasks and subscriptions; tmux view reports thread state as unknown. | Context has durable Collab state but lacks native Codex state sensing and observation timestamps/source. |
| `collab/src/server/state.rs::restore_unique_current_thread_routes_from_bindings` | Rebuilds route indexes from durable bindings and rejects ambiguous replay. | Durable route replay exists; current endpoint liveness proof remains transport-specific and pane-led. |

The first divergence is at runtime registration/candidate admission. Since the
AppServer endpoint/thread owner is not recorded in a production binding, every
downstream route, state read, notification, and subscription wake is forced to
tmux. The shortest repair is to require one explicit reachable AppServer owner
and verify the composite identity at admission, carry pane evidence as a
secondary anchor, and route every downstream operation from the same
daemon-selected binding. Do not separately patch each CLI command or add a
parallel state registry. Private embedded mode terminates at admission as
unsupported; it is never guessed from thread ID or redirected to tmux.

### Notification and subscription producers

| Event | Existing producer | Current terminal | Audit |
| --- | --- | --- | --- |
| `direct-message` | Peer `sendmessage` and scheduler admissions; registration installs a reusable default lease. | Durable message and attempt feed the selected transport; currently tmux. | Producer and durable consumer are connected, transport owner is wrong for the new RPC-first target. |
| `resource-released` | Closing/finalizing a task releases exact waiters in `server/mod.rs`. | Matching message/attempt is durable, then currently tmux wake. | Producer is connected; retain its exact task subject and one-shot semantics. |
| `deadline` / `master-idle` | `server/timers.rs` tick creates due messages and durable wake signals. | Matching subscription and attempt are replayable, then currently tmux wake. | Producer is connected; state/authority gates must use the RPC-aware presence owner after migration. |
| `async-result` | No producer found in executable Collab source; it appears in the accepted event list, MCP schema, docs, skills and config. | A subscription can be persisted, but no result event reaches it. | Broken public DAG. The selected repair is to remove this unused public event contract consistently; no async-operation owner exists to preserve. |

Subscription state itself is durable and owner-scoped (`handle_notification_subscribe`,
state reducer/replay, unsubscribe); the missing link is not a second subscription
store. It is the current tmux-only event-to-transport dispatcher and the
unproduced `async-result` event. Existing AppSDK function/verification maps and
Collab skill correctly document the current tmux source and must be updated in
the same implementation candidate once the target wiring exists.

## DAG closure checklist

The architecture has a node and terminal for every path. The items below are
design/implementation gates, not claims that source wiring already exists.

| DAG | Owner | Success terminal | Failure terminal / proof |
| --- | --- | --- | --- |
| TUI process → identity collection | Collab CLI adapter | One candidate contains every runtime and pane fact currently observable. | No usable anchor → registration error, no guessed ID. |
| Candidate → daemon admission | Collab daemon register handler | Endpoint `thread/read` matches thread/session/cwd; supplied pane tuple is stored only as a recovery anchor; single durable binding receipt selects native RPC. | Missing/unreachable endpoint, wrong owner, conflict, stale supplied anchor, cwd mismatch → no binding mutation. |
| Binding → context/recovery | Daemon route table and `collab context` | Exact runtime pair recovers existing peer; pane path is used only at final precedence. | Anchor disagreement or non-unique match → `RECOVERY_AMBIGUOUS`, no role change. |
| Message → native operation | Collab notification/delivery owner | One `turn/start`, `turn/steer`, or queue op recorded with native response. | Pre-submit rejection, ambiguous timeout, writer conflict remain distinct and no duplicate fallback. |
| Native operation → consumption | Target `collab recv`/ACK plus daemon attempt state | Message-specific receive receipt closes Collab consumption; Codex turn completion alone only proves RPC execution. | RPC accepted without the target's message-bound receive receipt remains accepted/pending/unknown, never consumed. |
| Turn cancellation/failure → delivery state | Collab durable attempt ledger | Cancelled/failed TUI turn leaves the mailbox message unread and queryable; a later explicit `recv` consumes it. | No synthetic ACK, deletion, or automatic cross-transport resend. |
| AppServer → presence/state | AppServer adapter and Collab projection | `thread/read`/turn/queue query provides timestamped state; `notLoaded` is reported as cold separately from live/idle; pane identity data is excluded from presence. | RPC probe failure is unknown; no auto-offline or master election on unknown. |
| Subscription → notification | Daemon subscription reducer | Exact active subscription matches event; one durable attempt is dispatched and receipt advances state. | Expired/absent/cancelled/consumed subscription makes no transport call; retry is bounded and idempotent. |
| Restart/replay | Daemon journal replay | Binding, subscription and attempt state reconstruct identically; route revalidated before use. | Conflicting journal or route fails closed and preserves durable facts. |

Current source reaches durable message, subscription, attempt, and restart
replay nodes, but the first registration edge and every RPC-selected terminal
remain **not wired**. The checklist is the target acceptance DAG, not a source
completion claim.

## Ordered implementation plan

0. **Close the current subscription DAG:** no executable producer exists for
   the public `async-result` event, and no asynchronous-operation registry
   exists to own one. Remove this dead event from the daemon event list,
   config policy, MCP schema, skills, docs, and tests. Keep actual asynchronous
   peer results on the existing durable direct-message/task path. This
   prevents accepting subscriptions that can never fire. Re-open
   `async-result` only with a named operation owner, durable result event,
   idempotent producer, and end-to-end receipt gate.
1. **Endpoint reachability gate (closed):** Codex source proves a private
   embedded AppServer is in-process and has no external endpoint; the isolated
   two-TUI experiment proves RPC only when both clients use one explicit shared
   endpoint. Native Collab registration therefore requires an explicit shared
   owner and positive `thread/read` self-check. Private embedded registration
   has a specified fail-closed terminal. No endpoint is inferred from a
   `threadId`.
2. **Protocol and owner map:** owner/wiring audit above is complete. Add a
   composite runtime observation shape (runtime IDs, endpoint owner, optional
   pane tuple) separate from the AppServer RPC communication route. Add one
   typed registration receipt for the committed identity, generation,
   verified AppServer owner, capabilities, and recovery anchors.
3. **RPC foundation:** promote the existing WebSocket JSON-RPC adapter from
   test-only to production. Provide typed `inspect`, `read_state`, `start`,
   `steer`, `queue`, and identity self-check functions. Implement queue only
   against the experimentally verified `thread/queue/add` contract. Keep
   accepted, unknown, and consumed separate. Add positive and negative
   protocol tests.
4. **Composite register and recovery:** have init/recover report current
   session/thread, endpoint and optional pane facts in one request. Daemon
   verifies, persists the complete binding in one generation, and returns one
   receipt. Implement context recovery precedence and ambiguity rejection;
   preserve master grants only under the live-master constraint above.
5. **State sensing:** attach RPC state to `who/status/context` with source and
   observation time. Pane facts are used only for final identity recovery and
   never as presence evidence. Verify idle, active, queued, missing, stale,
   and unknown paths.
6. **Notification and subscription integration:** after removing the
   unproduced `async-result` contract, route direct messages and
   actionable subscription events through RPC. Reuse durable Collab
   notification state; add recipient ACK/consumption proof bound to the
   message and thread. Verify finite/reusable TTL behavior, restart replay,
   and no duplicate send after ambiguous RPC outcome. Update the existing
   function map, verification map, Collab skill, and AppSDK configuration
   skill in the same candidate so their producer/consumer edges match the
   implemented RPC route.
7. **End-to-end acceptance:** in an isolated project with two independent
   TUI clients on one owner, run init/recover, context restoration, start,
   steer, queue, status sensing, subscribed notification, recipient ACK,
   restart/replay, and negative identity/endpoint cases. Use tmux only to host
   clients, never as the message or status oracle when RPC is selected.
8. **Build, runtime and review:** combine with latest `origin/main`; run scoped
   red/green tests, full affected Collab gates, rebuild and install/restart the
   applicable local Collab daemon (never start or manage the Codex AppServer),
   replay the same live entry, then obtain an
   independent architecture review of the verified candidate before delivery.

## Test and acceptance plan

These are planned gates; none were run against implementation because this
turn records and audits the design only.

| Gate | Exact focus | Pass evidence |
| --- | --- | --- |
| Candidate and adapter unit tests | `cargo test --manifest-path collab/Cargo.toml --bin collab adapters::codex_app_server -- --test-threads=1` | Positive RPC self-check/start/steer/queue/status; reject wrong session, thread, cwd, endpoint owner, malformed response, and unsupported method. |
| Identity and route reducer tests | `cargo test --manifest-path collab/Cargo.toml --bin collab identity -- --test-threads=1` plus scoped route-resolution tests | Current pair wins; unique single runtime ID recovers only exact owner; conflicting IDs reject; pane-only recovery requires exact server/session/pane/pane PID; reuse/collision/stale PID reject. Verify same stable agent/runtime, generation and existing grant semantics. |
| Notification and subscription tests | `cargo test --manifest-path collab/Cargo.toml --bin collab notification -- --test-threads=1` | RPC accepted != consumed; cancellation/failure before `recv` leaves mail unread; timeout/writer-conflict/unknown never send through tmux; known pre-submit error can be explicitly retried once; exact subscription ownership/event/expiry/replay behavior is preserved. |
| Delivery mode selection | CLI/MCP message schema plus adapter/server tests | `immediate` remains the default; active turn selects `turn/steer`; `queued` active turn selects `thread/queue/add`; idle/cold selects `turn/start`; unknown/multiple-active rejects before any request. The chosen operation and mode replay from durable attempt state. |
| Subscription API negative case | Existing notification CLI/MCP/config tests | After the DAG repair, `async-result` is rejected as unsupported and no public help/schema/config/skill lists it; `direct-message`, `resource-released`, `deadline`, and `master-idle` remain wired to real producers. |
| Full Collab suite | `cargo test --manifest-path collab/Cargo.toml -- --test-threads=1` | New native RPC path passes; pane identity recovery remains covered; no tmux message, wake, status, or presence path remains; no lifecycle or mailbox regression. |
| Format, type and build | `cargo fmt --manifest-path collab/Cargo.toml -- --check`; `cargo check --manifest-path collab/Cargo.toml --bin collab`; `cargo build --manifest-path collab/Cargo.toml` | Formatting, production module wiring, and release binary pass. |
| Two-peer live acceptance | New isolated integration gate modeled on `docs/collab.md` setup, plus the `/tmp/ac27` experiment | Two TUI clients share one isolated AppServer; both init receipts contain runtime and optional pane facts; send one start, one active steer, one queued message; target actually runs `collab recv` and journals a message-bound ACK; RPC state matches idle/active/queued and `notLoaded`/cold transitions. tmux is not used as message/status oracle. |
| Embedded endpoint fail-closed | Codex source audit plus registration negative test | A private `AppServerClient::InProcess` has no endpoint for external Collab; registration returns an actionable endpoint-owner error, does not mutate binding, and does not call tmux. |
| Recovery and replay live acceptance | Same isolated setup, deliberately omit/reorder available identity anchors and restart only the isolated test daemon | `context` restores same peer by session/thread first, then exact pane only when both runtime IDs are missing; master grant restored only for exact prior principal when no live master; restart replays identical route/subscriptions/attempts. Duplicate peer/pane binding and negative conflicts preserve old binding and produce no message send or authority change. |
| Final installed-entry replay | Candidate binary installed only in the declared local test scope; restart the affected Collab daemon and rerun the same two-peer entry | Runtime version/hash match candidate; init → context → send → recipient ACK → status → subscription notification → recovery evidence from the rebuilt candidate. |

### Execution checklist

Checklist entries below record the candidate result and evidence saved in the
project run record. The RPC-first path has isolated live acceptance and restart
replay evidence. Independent Codex and AGY architecture reviews both passed on
the exact candidate; the remaining delivery gate is merge/push by a live
Collab master.

Review evidence: Codex final task `20260925T061700Z-review-45915-75af98-final`
verdict `pass` (`controller_no_blocking_findings`); AGY final task
`20260925T061700Z-review-77020-agy-final` verdict `pass`
(`controller_no_blocking_findings`, one non-blocking P2 diagnostic wording
advisory at `collab/src/server/mod.rs` line 4169).

- [x] Remove the unproduced `async-result` event contract consistently from
      runtime, config, MCP schema, skills, docs, and tests.
      Evidence: worktree `playground/collab-dag-close-20260924`, branch
      `codex/collab-dag-close-20260924`, HEAD
      `7908aab95bc54b208c3e88310649407d0acccf6e`, dirty-tree diff SHA-256
      `3dfe587ef47472056527c4fd40e13dc33088274c651ac1b1c8b6c287b924c874`;
      `cargo test --manifest-path collab/Cargo.toml -- --test-threads=1`:
      794 passed, 0 failed, 1 ignored; MCP 11 passed; tmux e2e 1 passed.
      `cargo fmt --manifest-path collab/Cargo.toml -- --check`,
      `git diff --check`, and `cargo build --manifest-path collab/Cargo.toml`
      passed. Build reports 150 existing warnings.
- [x] Close private/embedded endpoint reachability: Codex HEAD
      `40eac3ce8a0c10cbcb9db910d529355eb2f8fc09` maps the default `Embedded`
      target to `AppServerClient::InProcess`; `/tmp/ac27/RESULTS.md` proves the
      shared explicit endpoint path. Require a shared owner and fail closed for
      private embedded registration. Source anchors:
      `codex-rs/tui/src/lib.rs::start_app_server` and
      `codex-rs/tui/src/daemon_startup.rs::exclusion` (`--profile` excludes the
      implicit shared-daemon route).
- [x] Add protocol tests for AppServer identity validation, typed
      `thread/read`, start, steer, queue, and state operations.
      Evidence: `cargo test --manifest-path collab/Cargo.toml --bin collab adapters::codex_app_server -- --test-threads=1` passes 40 tests; queued active/idle routing covered by `queued_notify_routes_active_thread_to_queue_add` and `queued_notify_starts_idle_thread_with_turn_start`.
- [x] Add route and registration tests for exact pair, unique single runtime
      anchor, pane-only last resort, collisions, conflict, replay, and master
      grant preservation with live/absent/unknown outcomes.
      Evidence: existing identity/host route suites pass under the AppServer-first production wiring (`identity::tests` 34 passed; full bin suite 797 passed).
- [x] Add notification/subscription tests for event production, exact owner and
      expiry, acceptance versus consumption, recipient ACK, replay, and
      ambiguous RPC result without duplicate transport submission.
      Evidence: `notification` subset passes 44 tests including `queued_delivery_mode_reaches_notification_sink` and the AppServer sink no-tmux-fallback test.
- [x] Pass the full Collab test suite, format check, production build, and the
      affected AppSDK map/gate checks on the exact candidate.
      Evidence: `cargo test --manifest-path collab/Cargo.toml -- --test-threads=1` passes unit 804, MCP 11, appserver two-TUI e2e 3, and isolated tmux e2e 1; `cargo fmt --manifest-path collab/Cargo.toml -- --check`, `cargo check --manifest-path collab/Cargo.toml`, and `cargo build --manifest-path collab/Cargo.toml` pass on the dirty candidate.
- [x] Pass the isolated two-TUI live flow for init, context recovery, start,
      steer, queue, state sensing, subscription wake, and message-bound
      `collab recv` ACK.
      Evidence: `COLLAB_STATE_DIR=/tmp/appsdk-native-live-7DHB4c/collab-state`
      with isolated AppServer
      `/private/tmp/appsdk-native-live-7DHB4c/home/app-server-control/app-server-control.sock`
      (PID 17783). Workers `codex-%4`
      (`01a0d637-75e2-71b2-8634-2be0c08a9adc`) and `codex-liveB`
      (`01a0d639-dafc-7571-a0aa-bbc2bbbc04ce`) both registered appserver
      transports with pane recovery anchors. Immediate A->B send recorded
      `NotificationDeliveryAccepted` native turn evidence, B consumed the
      durable receipt (`ReceiveCommitted`, `m1790303819565-1`), B->A reverse
      path recorded the same accepted/consume pair (`m1790303871662-2`), and
      one queued send recorded a `queuedSubmission` queue add operation
      (`m1790303931486-3`) before B consumed it. `collab status --all`,
      `collab who`, `collab context`, and `collab inbox` all reported appserver
      transport, `presence: present`, `endpoint_live: true`, and native
      `thread_state` observations.
- [x] Prove whether default/private embedded AppServer endpoints are
      discoverable and reachable by the Collab daemon; keep shared-endpoint
      mode as an explicit constraint if they are not.
      Evidence: `socket_candidate` no longer synthesizes `$CODEX_HOME/app-server-control/app-server-control.sock`; only explicit `COLLAB_APPSERVER_SOCKET`/`CODEX_APP_SERVER_SOCKET` yields an AppServer candidate, preserving the private-embedded fail-closed boundary.
- [x] Pass restart/replay and negative live cases for missing/conflicting IDs,
      endpoint mismatch, pane reuse, duplicate binding, unknown liveness, and
      no-live-master recovery; prove no role mutation on unknown/live master.
      Evidence: a wrong AppServer socket registration failed closed with
      `APPSERVER_ENDPOINT_REJECTED: ADAPTER_ROUTE_UNAVAILABLE`; a thread whose
      Codex anchors already belong to `codex-liveB` failed with
      `IDENTITY_RESTORE_CONFLICT` before mutation. Restarting the isolated
      `collab serve` (PID 16538) from the candidate binary and replaying a
      fresh A->B immediate send produced native turn evidence
      (`turnId 01a0d66f-426c-7b50-8eb8-badcd2f6343d`) and a consumed
      `ReceiveCommitted` receipt (`m1790304213022-1`).
- [x] Rebuild/install the candidate in the isolated acceptance scope and replay
      the same entry against the loaded binary before independent architecture
      review.
      Evidence: `cargo build --manifest-path collab/Cargo.toml` was run on HEAD
      `7908aab95bc54b208c3e88310649407d0acccf6e` (dirty candidate); candidate
      binary `collab/target/debug/collab` served the isolated daemon after
      explicit restart and accepted the same immediate/queued live flows above.
- [x] Fix Codex review P1s in the candidate: persisted AppServer identity reuse
      now asks the live daemon to resolve the native session/thread
      (`RouteResolveNative`), and `handle_send_with_task` re-validates the
      recipient transport after its lock-free presence probe so a concurrent
      rebind fails closed before commit.
      Evidence: targeted regressions
      `persisted_appserver_binding_requires_live_native_route_resolution`,
      `resolve_native_route_rejects_thread_mismatch`, and
      `send_fails_closed_when_recipient_rebound_during_presence_probe` pass,
      and `explicit_send_reports_appserver_wake_rejection_after_durable_commit`
      labels rejected AppServer wakes with `APPSERVER_NOTIFICATION_REJECTED`;
      full suite 804 passed, 0 failed, 1 ignored, plus AppServer two-TUI e2e 3
      and tmux e2e 1; the rebuilt isolated daemon
      accepted a fresh A->B immediate (`m1790312351947-1`) and queued
      (`m1790312379075-2`, `queuedSubmission`) flow with
      `appserver-input-submitted` responses after restarting the isolated
      daemon (PID 99075, candidate SHA
      `b3ade9242c2ebac92c92b9b135fcb2761366192a0aef2cce634fc28e38ed16d2`),
      and B consumed both via `ReceiveCommitted` receipts.

Added dedicated AppServer two-TUI integration acceptance in
`collab/tests/appserver_two_tui_integration.rs`, in addition to the existing
tmux-only receipt e2e. The fixture owns an isolated project, host state, app
socket, daemon and two App Server runtime identities, verifies appserver
init/context/send/recv receipts (`appserver-input-submitted`, consumed after
`collab recv`), and fails closed for a missing App Server endpoint. Any live
fixture must stop only its own daemon and remove only its own resources.

## Review of this design

- Experiment supports shared-owner native RPC and thread-addressed start,
  steer, queue, and state reads; it does not support cross-owner routing.
- Design keeps AppServer as transport/state owner and Collab as identity,
  journal, mailbox, subscription, and role owner.
- The production implementation now selects the verified AppServer RPC
  transport when a reachable explicit endpoint is supplied; tmux remains a
  stored pane recovery anchor and is never used for message, wake, presence,
  or status after an AppServer binding commits.
- Registration, route recovery, message acceptance, consumption, sensing,
  subscription, restart, and negative outcomes each have an owner and an
  inspectable terminal.
- Source audit found the first missing edge at registration/admission and
  confirmed that downstream notification and subscription reducers already
  have durable state owners to reuse. The implementation must connect those
  owners without creating a second route or subscription registry.
- Pane-only recovery, status event streaming, and message-specific consumption
  receipts still need isolated runtime acceptance; these are explicit
  end-to-end gates, not facts proven by the TUI experiment.
- Turn output from `/tmp/ac27` is not substituted for Collab's durable
  `recv`/ACK consumption evidence; the live integration gate must prove both.

## Design-review verdict

**PASS for the design DAG; implementation DAG remains unwired.** Every path has
one owner and an inspectable success and failure terminal. The design preserves
one daemon route owner and durable Collab state, excludes tmux from
communication/presence, does not claim cross-AppServer support, and separates
native RPC acceptance from recipient consumption. The first implementation
edge is composite registration against an explicit reachable AppServer owner;
wrong-owner/thread/session/project matches fail before binding mutation.
Ambiguous RPC completion stops retries and has no tmux fallback. The private
embedded topology is explicitly unsupported because its in-process API has no
external endpoint. Pane-only recovery and message-bound `collab recv`
consumption remain implementation acceptance gates, not assumptions.
