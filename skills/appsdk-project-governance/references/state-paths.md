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
