# AppSDK M1 host persistence inventory

This inventory is the path-level input for the M1 migration. It records what
AppSDK owns, what another subsystem owns, and what must remain untouched. A
path is not disposable because it is old or has a stale timestamp.

Snapshot date: 2026-09-10 (America/Los_Angeles)

## Classification

| Exact path | Owner | Observed state | Class | Disposition and evidence |
| --- | --- | --- | --- | --- |
| `/Users/fanzhang/.appsdk/config.toml` | AppSDK/Collab integration configuration owner | Present; notification batch and runtime policy configured | retain | Keep as the host configuration source; verify after install. Do not copy it into a project or rewrite it during registration. |
| `/Users/fanzhang/.appsdk/projects.jsonl` | AppSDK | Absent before M1; created on the first successful registration | current | Append-only host project registration source. Snapshot line count/digest before any future migration. |
| `/Users/fanzhang/.appsdk/projects.jsonl.lock` | AppSDK | Absent before M1; created when a writer registers a project | current control | Keep while the registry is in use; remove only as an exact stale lock through an authorized recovery procedure. |
| `/Users/fanzhang/.cargo/bin/appsdk` | AppSDK release installer | Current canonical `0.1.6` executable | retain | Replace only through `scripts/install-global-appsdk.sh`; verify version and digest. |
| `/Users/fanzhang/.local/bin/appsdk-beta` | AppSDK release installer | `0.1.4-beta.1` legacy executable | unknown/legacy | Snapshot metadata and obtain explicit cleanup authorization before removing. Installer does not implicitly delete it. |
| `/Users/fanzhang/.local/bin/appsdk.backup.20260904233029` | AppSDK release installer | Timestamped `0.1.6` backup | retain until rollback decision | Preserve until the release owner records that rollback is unnecessary; then clean the exact path with a receipt. |
| `/Users/fanzhang/.local/share/appsdk/0.1.0`, `0.1.2`, `0.1.3`, `0.1.4`, `0.1.4-beta.1` | AppSDK release installer | Versioned historical bundle directories | retain until release cleanup | These are install artifacts, not runtime registry state. Remove only exact version directories after rollback review. |
| `/Users/fanzhang/.local/share/appsdk/0.1.6` | AppSDK release installer | Current bundle | retain | Verify manifest/hash against the merged release; never use it as an unreviewed source. |
| `/Users/fanzhang/.agents/skills/appsdk-project-governance` | AppSDK Skill owner | Active global Skill | retain/migrate | Align the installed Skill with the reviewed source and bundle; preserve one owner. |
| `/Users/fanzhang/.codex/skills/appsdk-project-governance` | Codex Skill link | Symlink to the `.agents` Skill | retain | Keep the link; update the target through the official installation path. |
| `/Users/fanzhang/Documents/github/appsdk/.appsdk-control/long-task-goal.json` | AppSDK project-local runtime | Legacy goal state has `desired=recovery_required`, `observed=unknown` and `PROJECT_SCOPE_UNKNOWN` | retain as evidence, then canonical reset if authorized | Do not hand-edit or delete. Include in the migration snapshot; reset only through the named canonical command in a clean owner worktree. |
| `/Users/fanzhang/Documents/github/appsdk/.appsdk/` | AppSDK source checkout | Absent at the source root | not-a-managed-project | Do not run `appsdk init` merely to manufacture a consumer contract. The source repository remains an SDK release surface. |
| `/Users/fanzhang/Documents/github/appsdk/.agent-collab/server/` | Collab | Historical per-project daemon files, journal, events, logs, PID/socket/lock | retain/Collab migration | Collab owns this state. Use Collab's migration/recovery commands; AppSDK never deletes or edits it. |
| `/Users/fanzhang/Documents/github/appsdk/.agent-collab/mailbox/`, `runs/`, `claims/`, `handoff/` | Collab | Historical collaboration evidence | retain | Preserve JSONL, task, claim, and handoff evidence. Do not truncate or replay by copying. |
| `/Users/fanzhang/.local/state/collab/` | Collab | Global daemon socket/journal/events/log/PID | retain/Collab migration | Canonical Collab state; excluded from AppSDK registry/reset. Prove one daemon through Collab's owner commands. |
| `/Users/fanzhang/.codex/memories/` | Memory owner | Present | retain | Not AppSDK registry state; follow the memory owner and project-memory migration rules. |
| `/Users/fanzhang/.agents/skills/project-memory` | project-memory owner | Present | retain | Independent Skill; do not relocate or delete as part of AppSDK registration. |

## Required migration record

Before changing any retained or legacy path, append a migration record outside
the discard set containing:

```text
run_id, operator, owner, canonical project root, source/target version,
path, owner, class, observed status, size/count/digest, authorization,
canonical command, result, verification command, timestamp
```

The record must not contain credentials, identity tokens, or message payloads.
Unknown ownership or an in-flight operation blocks classification and deletion.

## Reset boundary

For a managed consumer project, an authorized reset may remove only the exact
legacy `.appsdk/` records/transactions/maps and declared local generated
projections, plus local `.appsdk-control/` state owned by that project. It does
not authorize removal of `active/`, `protected/`, business source, Collab
journal/mailbox/token/task/claim/binding, project-memory, release backups, or
other projects' worktrees. The AppSDK source repository is not a consumer
project and must not be reset through this boundary.

## Verification after migration

1. Re-read the inventory paths and confirm retained evidence has the expected
   count/digest.
2. Confirm the host registry contains one valid event per initialized canonical
   project and no credentials.
3. Run AppSDK verification and the installed-binary receipt replay.
4. Run Collab's own route/identity/restart replay separately; a registry event
   is not proof of a live TUI identity or bidirectional message.
