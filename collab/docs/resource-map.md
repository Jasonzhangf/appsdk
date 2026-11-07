# Collab current resource map

| Resource | Unique owner | Current truth | Boundary |
|---|---|---|---|
| Identity | `BindingControlStore` through mapped Context | `identity-map.json` | Select one scoped current name/binding. Exact credentials and grants serve that principal; archives and route indexes do not select identity. |
| Project authority | Current typed grant owner | Current grant in business state, referenced by the selected binding | User-approved promote/clear and authorized delegation; a name or endpoint does not grant master. |
| Current business persistence | `Server` current-state writer | `<route.storage_root>/.agent-collab/server/state.json` | Save the latest State directly; publish only the persisted candidate, with explicit prepublish failure or durability unknown. |
| Tasks and Board | Task/Board mutation owners | Current tasks, details, assignments, review/integration and cleanup records in State | Names own unfinished work; a carrier change preserves lifecycle obligations. Open views do not rewrite assignments. |
| Business block approval | Current task owner and authorized supervisor through Server | One current `task_block_requests` row per task | Pending is not approved blocked. Self/foreign/stale decisions fail; internal resource isolation is separate. |
| Resource claims and waits | Current task owner through Server | Active task feature/worktree claims and `TaskRec.wait` | Synchronous conflict result, bounded acyclic wait and exact subscribed release; no silent takeover. |
| Mailbox and receive | Message/receive owners | State messages, current name addresses and real receive receipts | Preserve body/sender/ID and real consumed state. Context/inbox/status do not consume. |
| Subscriptions and wakes | Registered subscription and notification owners | Current subscriptions, latest attempt/evidence/selected binding in State | Native acceptance is distinct from consumption. Unknown outcome does not justify blind resend or transport switching. |
| Explicit communication recovery | Mapped Context communication owner | Current message attempt binding, receive receipt and authenticated correlated reply | Known broken explicit requests only. Healthy/unknown and ordinary Context/restart stay silent. |
| Route and endpoint observations | `ProjectRuntimeManager` and adapters | Current map binding, route storage index and live observation | Locate the selected principal's transport; do not infer identity from cwd, display text or a historical route. |
| Agent context | Context projection owner | Current map and authorized State projection | Return current identity, own work, inbox and authorized management scope; no implicit consumption. |
| Managed compatibility records | Existing subagent owner | Current legacy child/task/mailbox records in State | Diagnostic facts grant no new execution authority. |
| Project reset | Current scoped reset owner | Current reset control intent and pending business reset | Preserve foreign scopes and live DSH as declared. Pending reset blocks rebuilding that scope until completion. |
| Daemon maintenance | User through the formal service entry | Declared service record, owned socket/PID/lock and current state | Agent role never grants lifecycle authority. Collab does not own tmux server lifecycle. |

No migration transaction, snapshot/replay maintenance owner or event history is
part of this resource map. Validation entry points are in [verification-map.md](verification-map.md).
