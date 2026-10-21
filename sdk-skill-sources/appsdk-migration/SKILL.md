---
name: appsdk-migration
description: "AppSDK/Collab 控制面迁移、context 身份与通信恢复、授权项目重置；遵循所属状态 owner，全局 daemon 始终由用户管理。"
---

# AppSDK & Collab Control-Plane Migration

## Scope and mandatory boundary

Use this Skill for requested control-plane migration, version installation, identity recovery or scoped reset. Ordinary feature development and rule editing do not require runtime migration.

- **No pre-release SHA or snapshot ceremony**: Follow [`AGENTS.md`](../../AGENTS.md) L2 and [`coding-principals/references/workflow.md`](../coding-principals/references/workflow.md#消融与校验取舍). Use normal command receipts, `--version`, daemon PID/socket liveness, and behavior replay. Do not add manual SHA/checksum manifests or multi-phase JSONL ceremony wrappers.
- **Strict owner separation**:
  - **AppSDK state** (`.appsdk/`, `.appsdk-control/`, `active/`, `protected/`) is owned by `appsdk`.
  - **Collab state** (identity, routes, mailbox and project runtimes under `~/.collab/`) is owned by `collab`. Legacy `<repo>/.agent-collab/` runtime data is migrated by that owner; the project marker is not persistence authority.
  - Neither tool may delete or rewrite the other's control plane.

## 1. Identity & route recovery (no reset, no restart)

For ordinary peer or master recovery, use the existing user-managed daemon:

```sh
collab context
# If required_fields are returned, provide only observed runtime facts once:
collab context --provide '{"session_id":"...","thread_id":"...","endpoint":"..."}'
```

- Context restores current identity, binding, route, receive lease and existing valid owned subscriptions together. cwd selects project; the stable name owns work, and the reported agent sessionID is its current carrier. Same-name session changes automatically update all unclosed board assignments. Explicitly retire the old rule that a tmux session selects peer identity: tmux sessions, socket paths, server PIDs and pane history are placement only. The CLI does not parse the `TMUX` value. Migration never requires restarting tmux or rewriting historical task evidence.
- An existing master does not block ordinary peer recovery. Name/session takeover follows the [Collab name contract](../collab/references/stable-name-recovery.md). Explicitly confirmed user takeover of unavailable unknown Jenny takes precedence over Jenny adjudication; do not wait for the old executor. Do not simulate recovery by changing a display label.
- Recovery acceptance includes real send, receive and reply. A role field alone is not communication recovery. Do not reset a project or operate daemon lifecycle for identity recovery.
- After recovery, evaluate owned patrol tasks and timed subscriptions. Close them when their task is complete or no new task and no actionable work remains; do not recreate idle patrol loops. Keep direct-message reception for new assignments.
- Master and peer each restore and manage their own goal under the current stable name. Recovery does not transfer a peer goal to master, require master proxy registration, or grant project-master authority to a peer.
- Do not hand-edit `routes.jsonl`, `identities/`, or `.agent-collab/` files.

## 2. Collab version upgrade & daemon maintenance

Installing a new binary (`scripts/install-global-collab.sh`) updates `$CARGO_HOME/bin/collab` and `collab-mcp` and refreshes the embedded Skill. Installation and activation are separate facts. The global daemon is user-managed; agents do not manage its lifecycle. Maintenance commands and credentials stay outside agent interfaces. After user-managed activation, inspect:

```sh
collab --version
collab status --all
collab context
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"codex","version":"0.0.0"}}}\n' | collab-mcp
```

Record old/new binary version, daemon PID/socket, `collab context` registration state, and MCP initialize receipt. Detailed procedures live in [`collab/references/migration-daemon.md`](../collab/references/migration-daemon.md) and [`collab/references/recovery-and-reset.md`](../collab/references/recovery-and-reset.md).

## 3. Control-plane migration vs. authorized reset

Choose the exact owner operation authorized by the user:

| Goal | Canonical command sequence |
| --- | --- |
| Restore a project with replayable legacy runtime data | `collab context`; daemon transfers the original runtime data to `~/.collab/projects/` under its writer lock and publishes one authoritative route; failure preserves source data |
| Current project master clears this project's history, routes, tasks, assets and load online | `collab reset --project`; preserve other projects and the global daemon |
| Current pane removes its own binding and route | `collab worker close --self`; unfinished tasks do not veto it, and the host process stays alive |
| Retire legacy AppSDK governance (`.appsdk-control/` / legacy `.appsdk/`) | Use the canonical `appsdk` reset/init owner (see [`appsdk-project-governance/references/bootstrap-migration.md`](../appsdk-project-governance/references/bootstrap-migration.md)); never deletes `.agent-collab/` |

## Hard redlines

- A peer orchestrates its long-horizon goal; bounded subworkers follow their assignment. Current-pane self removal and current-master project reset use the authority above. Historical identity protection adds no second approval.
- AppSDK/Collab does not own tmux lifecycle. Installation, activation, recovery and migration never stop or restart tmux or terminate its host panes.
- Never use `pkill`, `killall`, `rm -rf .agent-collab`, or a second daemon socket to work around migration or lock errors; preserve the exact error and escalate to the owner.
