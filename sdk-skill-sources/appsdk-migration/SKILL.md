---
name: appsdk-migration
description: "AppSDK/Collab 控制面迁移、身份上下文恢复或授权重置; 仅状态、daemon、身份或 reset 实际变化时使用。owner 全程记录证据; worker 只可查候选, 不可删状态/重启/改身份/未授权删除。"
---

# AppSDK & Collab Control-Plane Migration

## Scope and mandatory boundary

Use this Skill only when an operator explicitly requests control-plane migration, daemon version upgrade with maintenance window, identity recovery, or authorized control-plane reset. Ordinary feature development, bug fixing, and rule ablation do not use this Skill.

- **No pre-release SHA or snapshot ceremony**: Follow [`AGENTS.md`](../../AGENTS.md) L2 and [`coding-principals/references/workflow.md`](../coding-principals/references/workflow.md#消融与校验取舍). Use normal command receipts, `--version`, daemon PID/socket liveness, and behavior replay. Do not add manual SHA/checksum manifests or multi-phase JSONL ceremony wrappers.
- **Strict owner separation**:
  - **AppSDK state** (`.appsdk/`, `.appsdk-control/`, `active/`, `protected/`) is owned by `appsdk`.
  - **Collab state** (`~/.collab/` host truth and `<repo>/.agent-collab/` project durable state) is owned by `collab`.
  - Neither tool may delete or rewrite the other's control plane.

## 1. Identity & route recovery (no reset, no restart)

For ordinary peer bootstrap or projection recovery after daemon restart:

```sh
collab context
# If required_fields are returned, provide only observed runtime facts once:
collab context --provide '{"session_id":"...","thread_id":"...","endpoint":"..."}'
```

- Do not run `collab down/up` or `collab reset` to recover a peer identity.
- Do not hand-edit `routes.jsonl`, `identities/`, or `.agent-collab/` files.

## 2. Collab version upgrade & daemon maintenance

Installing a new binary (`scripts/install-global-collab.sh`) updates `$CARGO_HOME/bin/collab` and `collab-mcp` and refreshes the embedded Skill, but does not restart a live shared daemon. When an operator authorizes a maintenance window:

```sh
collab --version
collab status --all
collab down
collab up
collab context
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"codex","version":"0.0.0"}}}\n' | collab-mcp
```

Record old/new binary version, daemon PID/socket, `collab context` registration state, and MCP initialize receipt. Detailed procedures live in [`collab/references/migration-daemon.md`](../collab/references/migration-daemon.md) and [`collab/references/recovery-and-reset.md`](../collab/references/recovery-and-reset.md).

## 3. Control-plane migration vs. authorized reset

Choose the exact owner operation authorized by the user:

| Goal | Canonical command sequence |
| --- | --- |
| Migrate replayable legacy `.agent-collab/` state | `collab migrate inspect` -> `collab migrate plan` -> `collab migrate apply` -> `collab down && collab up && collab context` -> `collab migrate verify` |
| Retire & rebuild project Collab state (offline, requires approval) | `collab down` -> `collab reset --project --approval "<exact user text>"` -> `collab up` |
| Retire & rebuild host Collab index (offline, requires approval) | `collab down` -> `collab reset --host --storage-root <root> --approval "<exact user text>"` -> `collab up` |
| Retire legacy AppSDK governance (`.appsdk-control/` / legacy `.appsdk/`) | Use the canonical `appsdk` reset/init owner (see [`appsdk-project-governance/references/bootstrap-migration.md`](../appsdk-project-governance/references/bootstrap-migration.md)); never deletes `.agent-collab/` |

## Hard redlines

- Workers may inspect candidate state read-only (`collab context`, `collab status --all`, `appsdk verify`), but must never run `reset`, stop/restart the shared daemon, promote/clear master authority, or delete control directories without explicit user approval.
- Never use `pkill`, `killall`, `rm -rf .agent-collab`, or a second daemon socket to work around migration or lock errors; preserve the exact error and escalate to the owner.
