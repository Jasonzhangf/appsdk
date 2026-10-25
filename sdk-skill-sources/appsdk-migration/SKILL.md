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

Use the [Collab recovery contract](../collab/references/stable-name-recovery.md#恢复操作context-自己迁移agent-不做历史清理)
for current identity, name/session ownership, binding and receive state.
It owns the host-specific facts and takeover authority; this Skill does not
maintain another recovery protocol. Ordinary recovery uses the existing
user-managed daemon:

```sh
collab context
```

Reuse a successful result. Supply only actual missing facts requested by
`required_fields`, using the selected host contract; do not invent an endpoint,
choose a historical identity or require a second context call. Recovery
preserves owned work and unconsumed messages. It does not authorize reset,
daemon operations or hand-edited control state. Communication repairs verify
the affected send/receive/reply behavior; ordinary recovery adds no round trip.
Continue owned tasks and maintain only their necessary schedules through the
[Collab Skill](../collab/SKILL.md#目标编排与接续).

## 2. Collab version upgrade & daemon maintenance

The official `scripts/install-global-collab.sh` updates the installed binaries
and embedded Skill. Installation and activation are separate facts. The global
daemon is user-managed; agents do not manage its lifecycle. Maintenance commands
and credentials stay outside agent interfaces. Select affected checks through
[Collab verification](../collab/references/verification.md); a version query,
all-project status, context and MCP initialization are not a mandatory sequence.

For Desktop, keep the CLI entry; use the current Desktop bridge capability as
documented in [Desktop registration](../collab/references/migration-daemon.md#desktop-cli-registration-and-messaging).
Installing files does not activate a bridge service. Record
installed and loaded-runtime evidence separately; do not require an MCP tool
call or ask the user to find an assumed reconnect control.
Detailed maintenance boundaries live in [`collab/references/migration-daemon.md`](../collab/references/migration-daemon.md); ordinary identity migration remains internal to context.

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
