---
name: appsdk-migration
description: "AppSDK/Collab 控制面迁移、context 身份与通信恢复、授权项目重置；遵循所属状态 owner，全局 daemon 始终由用户管理。"
---

# AppSDK & Collab Control-Plane Migration

## Scope and mandatory boundary

Use this Skill when a user asks to move existing project governance, refresh
installed resources, recover a Collab binding, or discard an exact project
scope. Select one path from the current observed state.

| Current state | Action and owner | Verification |
| --- | --- | --- |
| Existing governed project has `.appsdk/project.json` and needs current managed resources | Project owner runs `appsdk init` and `appsdk verify` from that project root | Managed resources refreshed, existing project contract and records retained; confirm the affected consumer entry |
| Existing project is adopting AppSDK for the first time or changing its governance root | Project owner runs `appsdk prepare` in the intended project root, reads `.appsdk-prepare.json`, applies the approved scope, then runs `appsdk init` and `appsdk verify` | Use the [prepare contract](../appsdk-project-governance/references/bootstrap-migration.md#new-project) for fields. `PREPARATION_NOT_CONFIRMED` means resolve the draft before init |
| Project only needs old governance rules or checks retired | Project owner follows [governance ablation](../appsdk-project-governance/references/bootstrap-migration.md#existing-project-governance-ablation) against the live AGENTS, Skills and calls | Exercise the affected project command and check the retained behavior |
| Collab name, session or route requires recovery | Original affected host session uses `collab context` from its actual project root | Follow the [recovery table](../collab/references/stable-name-recovery.md#正常恢复) and check current name, binding and work ownership |
| Global Collab upgrade or restart after installation | Use [install and activate](../collab/references/migration-daemon.md#install-and-activate-separately) | Record the CLI version, daemon version and affected behavior separately |
| Explicit control-state discard | Select the named owner and project in the reset table below | Retain the actual reset receipt and verify the agreed scope |

- **No pre-release SHA or snapshot ceremony**: Follow [`AGENTS.md`](../../AGENTS.md) L2 and [`coding-principals/references/workflow.md`](../coding-principals/references/workflow.md#消融与校验取舍). Use normal command receipts, `--version`, daemon PID/socket liveness, and behavior replay. Do not add manual SHA/checksum manifests or multi-phase JSONL ceremony wrappers.
- **Strict owner separation**:
  - **AppSDK state** (`.appsdk/`, `.appsdk-control/`, `active/`, `protected/`) is owned by `appsdk`.
  - **Collab state** (identity, routes, mailbox and project runtimes under `~/.collab/`) is owned by `collab`. Retired project-local journals and mailboxes remain on disk and are not imported or replayed; central identity compatibility belongs to context. The project marker is not persistence authority.
  - Neither tool may delete or rewrite the other's control plane.

## 1. Identity & route recovery (no reset, no restart)

`collab context` owns automatic compatibility and migration of old identity,
binding, route, name/session and receive state. The agent never inventories
duplicate credentials, chooses a historical worker, closes old peers as a
prerequisite, or hand-edits the control plane. New identity semantics and the
user's explicit current recovery intent take precedence over old records.
Use the exact [recovery commands and receipt contract](../collab/references/stable-name-recovery.md#正常恢复).
Do not reverse-engineer approval fields or run a separate migration ceremony.

For ordinary peer or master recovery, use the existing user-managed daemon.
Desktop also uses Collab CLI. Its identity anchor is the current agent
sessionID; thread and bridge endpoint only locate communication. Agent-side
MCP calls are optional. Follow [Desktop CLI registration and messaging](../collab/references/migration-daemon.md#desktop-cli-registration-and-messaging)
for automatic current bridge discovery, the first-enrollment limit and receipts.
Use that reference for host-specific current facts; do not duplicate its
parameter contract here or supply an AppServer endpoint for Desktop.

- Context restores the stable name's work and current communication binding under the [Collab name contract](../collab/references/stable-name-recovery.md). An existing master does not block ordinary peer recovery. Recovery does not grant master authority or transfer another owner's goal.
- Explicit communication repairs and communication-feature delivery require real send, receive and reply evidence. Ordinary identity recovery adds no communication round trip. Do not reset a project or operate daemon lifecycle for identity recovery.
- After recovery, reuse valid owned goal/task schedules. Use [goal status and operations](../collab/operations.md#目标入口与参数) only when a specific schedule needs inspection, editing, ending, or authorized renewal. The current master's standing project patrol and per-task board-idle cancellation follow [board-idle patrol](../collab/references/board-idle-patrol.md).
- Do not hand-edit `routes.jsonl`, `identities/`, or `.agent-collab/` files.

## 2. Collab version upgrade & daemon maintenance

Installing a new binary (`scripts/install-global-collab.sh`) refreshes the
installed products and embedded Skill. Installation and activation are separate
facts. The global daemon is user-managed; agents do not manage its lifecycle.
Maintenance commands and credentials stay outside agent interfaces. Select
affected checks from [verification](../collab/references/verification.md),
reuse valid receipts and query only a concrete missing fact.

For Desktop, keep the CLI entry; use the current Desktop bridge capability as
documented above. Installing files does not activate a bridge service. Record
installed and loaded-runtime evidence separately; do not require an MCP tool
call or ask the user to find an assumed reconnect control.
Detailed maintenance boundaries live in [`collab/references/migration-daemon.md`](../collab/references/migration-daemon.md); ordinary identity migration remains internal to context.

## 3. Control-plane migration vs. authorized reset

Choose the exact owner operation authorized by the user:

| Goal | Canonical command sequence |
| --- | --- |
| Restore current identity and work | `collab context`; preserve central state and reconcile historical identities there; retired project-local runtime data is not imported or replayed |
| Current project master clears this project's history, routes, tasks, assets and load online | `collab reset --project`; preserve other projects and the global daemon |
| Current pane removes its own binding and route | `collab worker close --self`; unfinished tasks do not veto it, and the host process stays alive |
| Retire legacy AppSDK governance (`.appsdk-control/` / legacy `.appsdk/`) | Use the canonical `appsdk` reset/init owner (see [`appsdk-project-governance/references/bootstrap-migration.md`](../appsdk-project-governance/references/bootstrap-migration.md)); never deletes `.agent-collab/` |

## Hard redlines

- A peer orchestrates its long-horizon goal; bounded subworkers follow their assignment. Current-pane self removal and current-master project reset use the authority above. Historical identity protection adds no second approval.
- AppSDK/Collab does not own tmux lifecycle. Installation, activation, recovery and migration never stop or restart tmux or terminate its host panes.
- Never use `pkill`, `killall`, `rm -rf .agent-collab`, or a second daemon socket to work around migration or lock errors; preserve the exact error and escalate to the owner.
