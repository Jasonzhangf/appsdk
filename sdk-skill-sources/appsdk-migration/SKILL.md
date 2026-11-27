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

## 3. AppSDK version migration runbook (legacy `0.1.x` -> `0.1.0015`)

All legacy AppSDK consumers (`0.1.5`–`0.1.0014`) must migrate to the current
baseline (`0.1.0015`). Never guess which command to run. Inspect the actual
project state first, then execute the single matching row in the matrix below.

### Step 3.1 Read-only state inspection (run before any mutation)

```bash
cd /abs/path/project
appsdk version
git branch --show-current
git status --short --branch
test -f .appsdk/project.json && jq -r '{sdk_version: .sdk.version, stage: .lifecycle.stage}' .appsdk/project.json || echo "NO_PROJECT_JSON"
test -f .appsdk/sdk.lock && cat .appsdk/sdk.lock || echo "NO_SDK_LOCK"
test -f .appsdk/sdk-resources.json && jq -r '{schema_version, version}' .appsdk/sdk-resources.json || echo "NO_SDK_RESOURCES"
appsdk verify . || true
```

### Step 3.2 Exact state-to-action decision matrix

Important checkout boundary:
- Ordinary `appsdk init .` (without `--fresh`) rejects linked `git worktree` checkouts with `INIT_REQUIRES_CANONICAL_PROJECT_MAIN_TREE:<path>`; it must run in the canonical project checkout.
- Destructive reset `appsdk init "$PWD" --fresh --discard-legacy` must run in a clean non-`main`/`master` linked `git worktree`.

| Observed state | Exact action (no guessing) | Preserved vs. replaced | Post-check |
| --- | --- | --- | --- |
| `.appsdk/project.json` has `.sdk.version == "0.1.0015"`, `.appsdk/sdk-resources.json` is valid, and only managed `docs`/`rules`/`skills` need refresh | From the canonical project checkout on a clean non-`main` branch, run `appsdk resources refresh .`, then `appsdk verify .`. Do NOT run `--fresh`. | Business source, `AGENTS.md`, `contracts/`, records, `active/`, `protected/` preserved; `.appsdk/docs/`, `.appsdk/rules/`, `.appsdk/skills/` refreshed | `appsdk verify .` exits 0 |
| `.appsdk/project.json` has `.sdk.version == "0.1.0015"`, and `.appsdk/sdk-resources.json` or `.appsdk/contracts/` is missing/invalid | From the canonical project checkout, run `appsdk init .`, then `appsdk verify .`. Do NOT run `appsdk resources refresh` (it fails on missing/invalid `sdk-resources.json` and does not rebuild `contracts/`). | Business source, `AGENTS.md`, records, `active/`, `protected/` preserved; `.appsdk/contracts/`, `.appsdk/docs/`, `.appsdk/skills/`, `.appsdk/templates/`, and `.appsdk/sdk-resources.json` rebuilt | `appsdk verify .` exits 0 |
| `.appsdk/project.json` has `.sdk.version == "0.1.0014"` and clean canonical maps | From the canonical project checkout on a clean non-`main` branch, run `appsdk pin-lock . --binary "$(command -v appsdk)"`, then `appsdk init .`, then `appsdk verify .`. If `pin-lock` rejects tampered/unreconciled maps, stop and preserve the exact error before deciding whether to reconcile or request authorized `--fresh --discard-legacy`. | Project contract, records, `active/`, `protected/` preserved; `sdk.lock` and managed resources upgraded to `0.1.0015` | `jq -r .sdk.version .appsdk/project.json` is `0.1.0015` and `appsdk verify .` exits 0 |
| `.appsdk/project.json` has `.sdk.version` in `0.1.5`–`0.1.0013` (retired legacy baseline) or obsolete governance epoch | Obtain explicit user authorization naming the project and discard scope. Create a clean linked worktree from latest `origin/main` (`git worktree add -b codex/appsdk-migrate-<slug> <external-worktree-path> origin/main`), `cd` into it, and run: `appsdk init "$PWD" --fresh --discard-legacy` -> if Guidance is used: `appsdk guide init --task governance-reset --mode bootstrap --module <id>` & `appsdk guide compile` -> `appsdk verify .`. | Business source, runtime data, `active/`, `protected/`, `.agent-collab/` preserved; old `.appsdk/` records/pins/witnesses and declared `generated_root` rebuilt cleanly on `0.1.0015` | `appsdk verify .` exits 0; reset record has `mode: "fresh_init"` |
| `.appsdk/project.json` is absent (first-time adoption or relocated root) | From the canonical project checkout, run `appsdk prepare`, review `.appsdk-prepare.json` with the user until `status: "confirmed"` and `questions: []`, then run `appsdk init .` and `appsdk verify .`. Never synthesize or self-confirm `.appsdk-prepare.json`. | Existing source preserved; new `.appsdk/` created | `appsdk verify .` exits 0 |
| `.appsdk/project.json` is malformed or unreadable JSON | Stop. Do NOT delete `.appsdk/` or run `--fresh` blindly. Inspect the parse error, restore from Git history if accidental, or obtain explicit user authorization for fresh preparation. | Existing files untouched until owner decides | Exact parse error reported to owner |
| Governance is on `0.1.0015` and only old rules/SHA checks/CI hooks need retirement | Follow [governance ablation](../appsdk-project-governance/references/bootstrap-migration.md#existing-project-governance-ablation). Never run `--fresh` or `collab reset` for rule-only ablation. | Control plane untouched; legacy hooks/rules removed | Affected project entrypoint and CI checks pass |

### Step 3.3 Exact error-code recovery table

| Error code | Exact meaning | Required recovery action |
| --- | --- | --- |
| `PREPARATION_NOT_CONFIRMED` | `.appsdk-prepare.json` is missing, still `draft`, or has unresolved `questions` | Present `.appsdk-prepare.json` to the user, fill confirmed fields, then rerun `appsdk init .` |
| `INIT_REQUIRES_CANONICAL_PROJECT_MAIN_TREE:<path>` | Ordinary `appsdk init` was invoked inside a linked `git worktree` | Run ordinary `appsdk init .` in the canonical project checkout, or use authorized `appsdk init "$PWD" --fresh --discard-legacy` in the linked worktree |
| `MAIN_WORKTREE_MUTATION_FORBIDDEN` | Lifecycle/mutation command attempted on `main` branch | Switch to a clean non-`main` branch (or linked worktree for `--fresh`) and rerun the exact command |
| `GUIDANCE_MAIN_MUTATION_FORBIDDEN` | Guidance mutation attempted on `main` or `master` branch | Switch to a clean non-`main`/`master` owner worktree and rerun the exact command |
| `PROJECT_SDK_VERSION_PIN_MISMATCH:<v>:required_binary=appsdk-<v>` / `SDK_VERSION_MIGRATION_REQUIRED:<v>:0.1.0015:run=appsdk pin-lock <project> --binary <path>` | Project `.sdk.version` or `.appsdk/sdk.lock` does not match installed `appsdk` (`0.1.0015`) | Follow Step 3.2 row for `0.1.0014` (`appsdk pin-lock . --binary "$(command -v appsdk)"`) or `0.1.5`–`0.1.0013` (authorized `appsdk init "$PWD" --fresh --discard-legacy`) |
| `MISSING_SDK_LOCK` / `INVALID_SDK_LOCK` / `INVALID_SDK_LOCK_DIGEST` / `INVALID_SDK_BUNDLE_DIGEST` | `.appsdk/sdk.lock` is missing, malformed, or does not match the installed `0.1.0015` binary/bundle | Re-pin via `appsdk pin-lock . --binary "$(command -v appsdk)"` on a supported baseline, or run `appsdk init .` in the canonical checkout when `.sdk.version == "0.1.0015"` |
| `MISSING_SDK_RESOURCES` / `INVALID_SDK_RESOURCES` / `SDK_REVIEW_TEMPLATE_MISMATCH` | Managed `.appsdk/` bundle files or `.appsdk/sdk-resources.json` are missing or out of sync with `0.1.0015` | Run `appsdk init .` in the canonical project checkout (use `appsdk resources refresh .` only when `sdk-resources.json` is already valid and only `docs`/`rules`/`skills` need refresh), then rerun `appsdk verify .` |
| `LIFECYCLE_PRODUCER_MAP_TAMPERED:<map>` | Project `.appsdk/maps/<map>` diverges from canonical producer binding | Inspect `git diff .appsdk/maps/<map>`, restore or reconcile the declared map owner entry, never hand-forge hashes |
| `GUIDANCE_SETUP_REQUIRED` | Project opted into Guidance but has no compiled manifest yet | Run `appsdk guide init --task guidance-setup --mode bootstrap`, get approval for `GuidanceSetupProposal`, then `appsdk guide compile` |
| `REVIEW_ADMISSION_BLOCKED` | Required pre-review candidate/whitebox/deployment/blackbox evidence files are missing | Run the project's real adapters named in the `missing` JSON array; never hand-create records or retry unchanged state |

## 4. Control-plane migration vs. authorized reset

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
