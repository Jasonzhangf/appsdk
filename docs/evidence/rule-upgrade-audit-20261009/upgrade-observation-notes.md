# upgrade-observation notes

## Task and scope

- Task: `rule-upgrade-audit-20261009`, issue `aabed6a`.
- Identity: read-only independent observer. I do not edit product source, global rules, or other workers' resources. I do not dispatch agents.
- Goal: use the installed canonical `/Users/fanzhang/.cargo/bin/appsdk` (`0.1.0012`) to reproduce or refute the core report F1 claim on the smallest legitimate stale-pin consumer.
- Allowed writes: this directory only. Fixture files and evidence stay here for the planner.
- Product source and global state are read-only. I will not build a candidate binary, manipulate `~/.appsdk` or `~/.collab`, register/promote a master, send messages, or restart a daemon.

## Baseline

- Repository: `/Users/fanzhang/Documents/github/appsdk`
- HEAD: `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`
- Installed runtime: `/Users/fanzhang/.cargo/bin/appsdk`
- Runtime version: `appsdk 0.1.0012 (rust)`
- Runtime SHA-256: `f21c33a708cea0a0119690125577e87140c07e0a51af87ad30d5928220469b3b`
- F1 source chain read in this turn: `rust/src/main.rs:1453-1455` -> `rust/src/main/init.rs:886-897` -> `rust/src/main/governance.rs:247-292`; `write_current_sdk_lock` returns on version mismatch at `rust/src/main/init.rs:82-86`; `verify` rejects at `rust/src/main/project.rs:85-94`.

## Fixture plan

- Source fixture: `docs/evidence/user-requirement-truth-lock-20261007/session-lock/fixture-appsdk-0011.tar.gz`
- Source SHA-256: `b72bc517781d91bd609d27dce4f720659e251c6dbb60d720e3c2a1b09a6fcdc1`
- Why it is a legitimate old consumer: the archive contains a generated project whose `.appsdk/project.json` and `.appsdk/sdk.lock` both pin `0.1.0011`; its `.appsdk/sdk-resources.json` records 100 resources for bundle digest `sha256:54e6ba9f...`. It was retained as the `0.1.0011` migration fixture from the prior release task. I will not edit its governance contract before the first reproduction.
- Isolation: run the official CLI with `APPSDK_HOME` and `COLLAB_STATE_DIR` pointing at task-owned paths. Record any unavoidable external side effect.
- Commands planned:
  - `appsdk verify <fixture>` before mutation to record the stale-pin terminal.
  - `appsdk init <fixture>` and capture stdout, stderr, and exit code.
  - Snapshot and compare `.appsdk/project.json`, `.appsdk/sdk.lock`, `.appsdk/sdk-resources.json`, and installed bundle files.
  - `appsdk verify <fixture>` after `init` to record the terminal.
  - A current-version control fixture to show the same path does not trigger on a matching pin.
- Confirmation signal for F1: `init` exits 0, changes SDK-owned bundle resources and `sdk-resources.json` to `0.1.0012`, but leaves `.appsdk/project.json` and `.appsdk/sdk.lock` at `0.1.0011`; subsequent `verify` fails with `PROJECT_SDK_VERSION_PIN_MISMATCH:0.1.0011:required_binary=appsdk-0.1.0011`.
- Falsification signal: `init` fails before any SDK-owned resource write, or all version-bearing files move consistently to `0.1.0012`.

## Progress

- 2026-10-09T21:14:09-07:00 | baseline recorded | runtime and source baseline verified | no fixture command run yet.
- 2026-10-09T21:15:00-07:00 | fixture extracted | old consumer is valid and stale | `fixture-old-pin` from `fixture-appsdk-0011.tar.gz`; project and lock both pin `0.1.0011`; resources record `0.1.0011`; 196 archive entries.
- 2026-10-09T21:15:30-07:00 | environment adjustment | linked-worktree ancestor would trip an unrelated ordinary-init guard | `GIT_CEILING_DIRECTORIES=<fixture>` makes Git stop at the standalone consumer root. This does not alter the version/pin path. `collab` was omitted from `PATH` to avoid peer registration or daemon start; the optional `COLLAB_INIT_UNAVAILABLE` result is recorded below.
- 2026-10-09T21:15:45-07:00 | verify before | stale-pin terminal reproduced before mutation | exit 1, stderr `PROJECT_SDK_VERSION_PIN_MISMATCH:0.1.0011:required_binary=appsdk-0.1.0011`; `state/verify-before.*`.
- 2026-10-09T21:15:45-07:00 | ordinary init | F1 reproduced | exit 0; project and lock stayed `0.1.0011`; `sdk-resources.json` and installed bundle moved to `0.1.0012`; `state/init-old-pin.*`, `state/fixture-before.sha256`, `state/fixture-after.sha256`.
- 2026-10-09T21:16:00-07:00 | mutation delta | 7 existing files changed and 7 files added | changed bundle manifest, bundle skill/rule/template, resource map, and `sdk-resources.json`; added 0.1.0011 migration contracts and `user-requirement-request.schema.json`; `state/init-changed-existing.files`, `state/init-added.relative`.
- 2026-10-09T21:16:00-07:00 | verify after | failure remains | exit 1 with the same pin mismatch; `state/verify-after.*`.
- 2026-10-09T21:16:04-07:00 | current-pin control | no mutation and no pin mismatch | `appsdk new` then ordinary `init` changed zero files; `verify` exit 0 with `baseline_status=current`; `state/init-control.changed`, `state/verify-control.*`.
- 2026-10-09T21:16:30-07:00 | official migration control | `pin-lock` is the working migration entry | on a fresh copy of the same old fixture, `pin-lock` exit 0, project/lock/resources all reached `0.1.0012`, and `verify` exit 0; `state/official-*`.

## Observed result

### Ordinary `init` on the stale pin

Command:

```sh
env APPSDK_HOME="$STATE/appsdk-home" \
  HOME="$STATE/home" \
  COLLAB_STATE_DIR="$STATE/collab-state" \
  GIT_CEILING_DIRECTORIES="$FIXTURE" \
  PATH="/usr/bin:/bin:/usr/sbin:/sbin" \
  /Users/fanzhang/.cargo/bin/appsdk init "$FIXTURE"
```

Observed:

- exit `0`
- stdout ended with `initialized <fixture>`
- stderr recorded only the optional `COLLAB_INIT_UNAVAILABLE` message
- `.appsdk/project.json`: `0.1.0011` before and after
- `.appsdk/sdk.lock`: `0.1.0011` before and after
- `.appsdk/sdk-resources.json`: `0.1.0011` -> `0.1.0012`, 100 -> 106 resources
- installed bundle manifest: `0.1.0012`
- subsequent `verify`: exit `1`, same `PROJECT_SDK_VERSION_PIN_MISMATCH`

### Control and migration boundary

- A newly created `0.1.0012` project ran ordinary `init` with no file changes; `verify` exited `0`.
- A fresh copy of the same `0.1.0011` fixture ran official `pin-lock`; it exited `0`, wrote the `0.1.0011-to-0.1.0012` migration record and maps, and `verify` exited `0`.
- This confirms the trigger condition is the stale pin on ordinary `init`. It does not claim a product fix or an independent review.

## First divergence

- The first observable mutation occurs in `ensure_governance_layout` -> `bootstrap_contracts`: it adds the current missing root contract `contracts/records/user-requirement-request.schema.json` before any pin check.
- The main bundle pollution then occurs in `install_bundle_resources`: it unconditionally overwrites `.appsdk/contracts/**`, `.appsdk/docs/**`, `.appsdk/skills/**`, `.appsdk/rules/**`, `.appsdk/templates/**`, and `.appsdk/sdk-resources.json` with the current `0.1.0012` bundle.
- `write_current_sdk_lock` then sees `/sdk/version = 0.1.0011` and returns at `rust/src/main/init.rs:82-86`, so `.appsdk/project.json` and `.appsdk/sdk.lock` stay old.
- The missing preflight is in the `init_project` orchestration before `ensure_governance_layout` at `rust/src/main/init.rs:886`. The minimal owner suggestion is that function: check the existing project pin before any bundle write and fail closed with a migration-required code that points to `pin-lock`, or route the refresh through the existing migration transaction. Do not move migration ownership out of `pin-lock`.

## Evidence and resource ownership

- Owned fixture paths:
  - `fixture-old-pin/` (mutated by the reproduced ordinary `init`)
  - `control-current-pin/` (current-version control)
  - `fixture-old-pin-pinlock/` (post-init recovery probe)
  - `fixture-old-pin-official/` (clean old fixture migrated by official `pin-lock`)
- Owned evidence/state: `state/`
- Owned registry state: `state/appsdk-home*`; no global `~/.appsdk` path appears in the command receipts.
- No `.agent-collab`, `.mcp.json`, daemon, or global peer registration was created because `collab` was absent from the isolated `PATH`.
- Product source remained unchanged: repository `git status --short --untracked-files=all` was empty at HEAD `3dfdaf8503b7a6f6a76651a1e282c038b6648c3a`.
- Recycle condition: keep until the planner consumes the report and evidence. All listed fixture/state paths are task-owned and can be removed after that review.
