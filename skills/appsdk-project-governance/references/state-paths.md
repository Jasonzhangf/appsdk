# AppSDK State Paths and Components

## Global truth

Host-wide AppSDK governance and Collab truth are separate:

```text
~/.appsdk
  projects.jsonl
  runtimes.jsonl
  communication.jsonl
  config.toml

~/.collab
  server.sock
  daemon.lock
  server.pid
  events.jsonl
  log.txt
  routes.jsonl
```

Usage:

- `~/.appsdk` is AppSDK's global persistent truth. It is not a project
  directory. Do not hand-edit or delete its `.jsonl` files; AppSDK updates them
  through its commands and reset/migration lifecycle.
- `~/.collab` is Collab's host-level daemon truth. It is owned by Collab, not
  AppSDK. Do not hand-edit or delete its files; see the Collab skill's
  `state-paths.md`.
- Deleting a project root does not authorize deleting global truth entries.
  Global entries are retired through the owning lifecycle.

## Project-local AppSDK state

For a governed project root:

```text
<project>/.appsdk/
  project.json
  goal.json
  sdk.lock
  contracts/
  records/
  maps/
  guidance/
  skills/

<project>/.appsdk-control/
  run state
  temporary guidance/harness output
  local runtime state

<project>/.agent-collab/
  Collab project-local durable state
```

Usage:

- `.appsdk/project.json` is the project governance contract. After
  `appsdk init`, it contains placeholder `project_id: "change-me"`, `goal.json`
  contains `goal-change-me`, and the module scaffold contains `app-core`.
  Replace those with the real project contract before `appsdk verify`.
- `.appsdk-control/` is local runtime state and is not committed truth. It is
  removed or reset through AppSDK reset/init, not by hand-deleting arbitrary
  files.
- `.agent-collab/` is Collab-owned project-local durable state. AppSDK reset
  must not delete it; Collab migration/retirement owns it.

## Lifecycle commands and meaning

```text
appsdk prepare                 -> create/confirm scope and boundaries
appsdk init .                  -> scaffold/refresh governance and register project
appsdk guide compile           -> compile declared guidance after binding the contract
appsdk verify                  -> verify the current contract/baseline
appsdk reset-governance --discard-legacy
                               -> AppSDK control-plane reset
appsdk init --fresh --discard-legacy
                               -> preferred single transaction for old AppSDK control plane
```

For old `.appsdk/` state, do not delete it manually. Use the authorized reset
route after the Collab side is migrated or retired. `appsdk init --fresh
--discard-legacy` removes the AppSDK-owned old control plane and rebuilds the
current baseline; it does not delete `.agent-collab/` or global truth.

When the user explicitly asks to start fresh instead of migrating legacy
state, require the prior control plane to be removed first:

1. Have the prior AppSDK/Collab version remove or retire its project-local
   `.appsdk/`, `.appsdk-control/`, and `.agent-collab/` state.
2. Back up and prune stale host routes only through the documented
   route-cleanup path, then verify the global daemon with `collab up` and
   `collab status --all`.
3. Initialize from the current global baseline with `appsdk init --fresh
   --discard-legacy`, bind the real project contract, then `appsdk guide
   compile` and `appsdk verify`.

## Registration verification

### Where registration must run

`appsdk init` / `collab init` registers the **canonical project root** — the
main checkout of the project, not a worktree. The daemon rejects any init
attempt that runs from a `playground/<slug>` or any other worktree with
`collab init must run from the project main tree, not a ./playground
worktree`. If you are inside a worktree:

1. `cd` back to the canonical project root (the directory registered in
   `~/.collab/routes.jsonl` as `canonical_root`).
2. Confirm with `rg -n "\"canonical_root\":\"$(pwd)\"" "$HOME/.collab/routes.jsonl"`.
3. Re-run `appsdk init .` / `collab init` from that directory.

A worktree may still own its own scoped writes, task, or claim, but it
cannot register a new project route and it must never overwrite
`.agent-collab/` from inside the worktree. If a worktree needs its own
route, register a separate project root that explicitly names the
worktree path; do not bypass the main-tree check by hand-editing
`routes.jsonl`.

After `collab init`, do not stop at command success. Verify durable truth:

```sh
WORKER_ID="<worker_id returned by collab init>"
PROJECT_ROOT="$(pwd)"
rg -n "\"id\":\"${WORKER_ID}\"|\"worker_id\":\"${WORKER_ID}\"" \
  "${PROJECT_ROOT}/.agent-collab/server/journal.jsonl" | head
rg -n "\"canonical_root\":\"${PROJECT_ROOT}\"" "${HOME}/.collab/routes.jsonl"
rg -n "\"app_scope_id\"" "${HOME}/.collab/routes.jsonl" | head
ls -la "${PROJECT_ROOT}/.agent-collab" "${HOME}/.collab"
```

Required journal events, in order: `GlobalProjectRegistered`,
`GlobalRuntimeBound`, `Registered`, `NotificationSubscribed`,
`CommandCompleted`. All five must share the same `worker_id`. Missing any of
them means registration is incomplete; do not proceed, do not report as
initialized.

See [`init-prompts.md`](init-prompts.md) for copy/paste master and peer
initialization prompts, including the stale route cleanup and
`RUNTIME_BINDING_REJECTED` recovery paths.
