# Collab daemon lifecycle resilience audit

## Scope and evidence

Audit scope: the global host daemon from start intent through replay, readiness, request service, crash/restart, stop, and owned-resource cleanup. Peer identity recovery is a separate lifecycle.

- Source baseline: `origin/main` / `75c2ddd8184a21991c9c73d59c32de8eb4c9214d`.
- Installed runtime observed: `collab 0.2.0153`, binary SHA256 `cdba3e58a3c404a67074e27812e1df7cac2beb1c30609c0fd37108bf983aadfd`; PID `84516` maps to it. Primary and REST sockets existed; `collab status --all` responded. Unauthenticated REST returned 401, proving listener reachability only.
- Read-only service-manager inspection found no Collab/AppSDK launchd job in `launchctl list` or checked LaunchAgents/LaunchDaemons directories.
- REST source is in staged communication candidate `/Volumes/Intel/playground/appsdk/comm-rest-20260929-main`, base `75c2ddd8184a21991c9c73d59c32de8eb4c9214d`, staged tree `a6556a3fb79c2b40a4658c1022ec0a193b3f085c` (not merged to origin/main).
- No daemon lifecycle command, crash injection, host reboot, or shutdown-under-load test was run. No claim of live self-recovery is made.

## Current ownership and flow

`client::ensure_server` checks the `DOWN` marker, probes primary typed Ping and host lock, waits up to 30s for Starting, launches a detached process for safe Unavailable, and fails closed on Unknown. It is called only by `context`, `init`, `up`; ordinary `status`, `send`, and other RPCs connect directly. `spawn_server` returns after `Command::spawn`, without waiting or supervising. `collab serve` resolves its project root from the process cwd; a login service has no guaranteed canonical project cwd.

The daemon acquires host and legacy writer fences, prepares both socket paths, replays resident journal, opens the writer, restores leases and uncertain async delivery state, purges expired storage, restores project runtimes and reconciles routes, then binds primary and REST sockets and publishes PID. Individual external-runtime startup errors may leave not-ready routes; fatal route reconciliation aborts startup. Typed readiness checks only primary Ping. There is no REST health endpoint.

On stop, the daemon stops timer work, signals connection tasks, joins all tasks without a deadline, then best-effort removes matching socket/PID paths while discarding cleanup errors. REST requests do not share the primary cancellation receiver. Listener accept errors are logged/retried without classification or backoff; timer task results are ignored.

```mermaid
flowchart TD
  E([context / init / up]) --> D{DOWN marker?}
  D -- yes --> F([Explicit unavailable])
  D -- no --> P[Primary Ping + lock probe]
  P --> S{Availability}
  S -- alive --> A([Primary-ready response])
  S -- starting --> W[Wait up to 30s]
  W --> A
  W --> F
  S -- unavailable --> X[Spawn detached process]
  X --> L[Acquire host and legacy locks]
  L --> R[Prepare sockets and replay journals]
  R --> M[Restore leases, attempts, runtimes and routes]
  M --> B[Bind primary + REST; publish PID]
  B --> H[Serve requests and timers]
  S -- unknown --> F
  L --> F
  R --> F
  M --> F
  B --> F
  H --> G{Stop, signal, fatal exit or crash}
  G -- graceful --> C[Stop scheduler and signal tasks]
  C --> J[Unbounded join]
  J --> Z[Best-effort unlink]
  Z --> T([Exit; lock released])
  G -- crash/reboot --> K([OS releases locks; journal remains])
  T --> N{Later ensure_server caller?}
  K --> N
  N -- yes, eligible command only --> P
  N -- no --> I([Can remain unavailable while idle])
```

This current-state graph is observational and is **not** a valid self-recovery SESE DAG: it exposes partial outcomes and has no independent restart owner.

## Findings

### P1 — No continuous crash recovery

The detached daemon has no waiting parent and no host supervisor job was found. A crash/reboot releases locks but does not restart service. Only later `context`, `init`, or `up` can reach `ensure_server`; ordinary RPCs may fail and the daemon can stay down indefinitely while idle.

### P1 — Readiness overstates service availability

The primary Ping does not check REST, registered route recovery, or timer progress. The REST listener has no health endpoint. A successful Ping cannot prove the full communication contract is usable.

### P1 — Stop can hang and cleanup is unproven

Joining requests has no deadline and REST has no shared cancellation. One stalled request can retain the process, lock, and sockets indefinitely. Ignored unlink failures mean process exit does not prove endpoint cleanup.

### P2 — Partial route restore has no recovery owner

An individual runtime restore can be ignored while the host starts, leaving a not-ready route; fatal same-pane reconciliation instead aborts startup. The two outcomes are not represented in a single health model or repaired by a background owner.

### P2 — Listener/timer failures can silently degrade service

Permanent accept errors can loop without backoff; timer task failures are discarded. The process can keep the writer lock while useful work has stopped.

### Evidence boundary

The observed daemon answered Ping and `status --all`; prior `COLLAB_IDENTITY_ANCHOR_MISSING` / `IDENTITY_REBIND_UNPROVEN` is a caller identity failure, not daemon downtime. Historical logs are not attributed to PID 84516 without process-lifetime correlation.

## Target SESE design

Availability scope: a per-user macOS LaunchAgent supervises the service while the user session is loaded. Before login or after user-session termination is outside this contract.

There is one durable service record with `desired_state`, monotonic `generation`, and `service_scope_root`, atomically owned by CLI lifecycle commands under the host state root. On the first `up` with no record, `service_scope_root` is canonicalized from that command's exact project cwd. Later `up` from any project reuses the already-bound service root; it cannot switch the global daemon to whichever project happens to call. A different root requires an explicit scoped migration. The root belongs in the same truth record because the daemon's reducers need a project root, but launchd does not inherit that cwd. launchd is supervisor/actuator, not another desired-state store. Its `collab serve --service` entry reads the typed service record and uses the recorded canonical scope; it never guesses from launchd cwd or environment. If that root is missing/moved or conflicts with durable route ownership, startup is blocked with the recovery action preserved; no other project root is substituted. `up` commits `running` plus the bound root then bootstraps/reconciles the LaunchAgent. `down` commits `stopped` then boots it out and verifies process/socket/lock release. `context`/`ensure_server` can reconcile only when state is `running`; they never override `stopped`. Retire `DOWN` as a separate authority. Reconciliation serializes requests and discards actions whose generation is stale. Every startup attempt re-reads the current generation and scope root immediately before listener publication; if stopped, superseded, or inconsistent, it exits and releases its writer lease. The host lock remains the single-writer fence.

```mermaid
flowchart TD
  E([One CLI service-control request]) --> D[Atomically commit desired state + generation]
  D --> A[Reconcile per-user LaunchAgent]
  A --> O[One supervisor-owned lifecycle node: start, health, crash recovery, backoff, stop and startup cancellation]
  O --> X([Return desired generation + typed observed outcome])
```

This external graph has one entry and one exit. Start/monitor/retry loops are internal behavior of the single LaunchAgent owner; they are not cross-node graph edges. The exit outcome is `running-ready`, `running-degraded`, `stopped`, or `blocked`, always tied to the desired-state generation.

| Internal supervisor transition | Resource disposition | Required evidence |
|---|---|---|
| Start/restart | `collab serve --service` reads desired generation and canonical service scope from the host service record; new process acquires host and legacy locks before touching state | one owner holds each required lock; scope root is an existing canonical route owner |
| Lock busy/ambiguous | Exit blocked; do not alter socket, PID or journal | exact lock owner/error |
| Journal/replay failure | Preserve durable journals; close handles and release locks on process exit | replay error bound to generation |
| Route failure | Keep isolated route explicitly degraded or block core corruption; no alternate reducer | per-route health/readiness projection |
| Bind/readiness failure | Process exits; next attempt verifies lock release and classifies stale/ambiguous endpoint before cleanup | primary + REST probes, endpoint ownership, and service-root binding |
| Crash/host reboot | OS releases locks; old process does no cleanup; launchd restarts/relaunches; new process validates PID/socket ownership and replays | new PID owns lock and durable state replay succeeds |
| Stop during startup | `stopped` generation supersedes attempt; bootout cancels/reaps; attempt rechecks generation before publish | desired generation stopped; no process/lock/socket |
| Stop while serving | Close admission, drain/cancel primary and REST to a deadline, flush durable writes | bounded receipt; PID gone, lock free, owned paths absent |
| Restart throttle exhausted | Enter explicit blocked outcome; preserve state and ambiguous artifacts | exact cause, desired generation, operator action |

The daemon never restarts itself after fatal internal failure; it exits with cause and launchd is the only restart owner. Retry a request only if failure is proven before any request bytes were sent. Otherwise return an uncertain result and query by durable ID; never replay a possibly committed mutation.

## Fix order and acceptance

1. Implement the single durable desired-state record and LaunchAgent adapter; eliminate detached `spawn_server` as a lifecycle owner and retire `DOWN` authority. Test concurrent up/down and login reconciliation.
2. Readiness checks primary and REST plus resident replay and explicit per-route readiness/degradation.
3. Bound shutdown, cancel/drain both protocols, flush writes, verify inode-matched socket/PID removal and lock release.
4. Surface accept/timer failures; apply bounded retry/backoff to transient errors; fatal errors exit to supervisor; corruption/ambiguous ownership block.
5. Isolated process-level crash injection at each startup/commit/feedback/stop boundary, concurrent lifecycle controls, corrupted journal, stale/ambiguous socket, REST-only fault, timer fault, and restart-throttle exhaustion.

Acceptance requires automatic restart without a client request; latest-generation convergence for concurrent controls; one lock owner; readiness bound to both endpoints and route classification; durable delivery/task state survives crash without duplicates or false success; stop/recovery reach bounded outcomes with complete owned-resource cleanup; tests, candidate/review SHA, build/install digest, service identity, restart timestamp, real multi-peer replay and cleanup receipts are bound together.

## Disposition

**FAIL for continuous self-recovery.** Single-writer fencing, journal replay, and stale-socket checks are useful foundations, but a live PID/Ping does not prove recovery from process or host failure. This is an audit/design artifact; no daemon product code changed.
