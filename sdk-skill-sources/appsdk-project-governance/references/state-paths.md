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
  project registration/reducer input
```

Usage:

- `.appsdk/project.json` is the project governance contract. After
  `appsdk init`, it contains placeholder `project_id: "change-me"`, `goal.json`
  contains `goal-change-me`, and the module scaffold contains `app-core`.
  Replace those with the real project contract before `appsdk verify`.
- `.appsdk-control/` is local runtime state and is not committed truth. It is
  removed or reset through AppSDK reset/init, not by hand-deleting arbitrary
  files.
- `.agent-collab/` is Collab-owned project registration/reducer input. It is
  not the peer, route, mailbox, task, or liveness truth. AppSDK reset must not
  delete it; Collab migration/retirement owns it.
- A Git worktree contains tracked `.appsdk/` files from its main checkout, but
  does not inherit ignored `.agent-collab/` or `.appsdk-control/` state. The
  registered peer identity is inherited from the global Collab state by the
  current Codex sessionID/App Server thread; do not create a second project
  registration from a worktree.

## Lifecycle commands and meaning

```text
appsdk prepare                 -> create/confirm scope and boundaries
appsdk init .                  -> scaffold/refresh governance and register project
appsdk guide compile           -> compile declared guidance after binding the contract
appsdk verify                  -> verify the current contract/baseline
appsdk reset-governance <project> --discard-legacy
                               -> AppSDK control-plane reset
appsdk init --fresh --discard-legacy
                               -> preferred single transaction for old AppSDK control plane
collab down
collab reset --project --discard-legacy --approval "<user text>"
collab up
collab context
                               -> Collab-owned project control-plane reset
```

For old `.appsdk/` state, do not delete it manually. Use the authorized reset
route after the Collab side is migrated or retired. `appsdk init --fresh
--discard-legacy` removes the AppSDK-owned old control plane and rebuilds the
current baseline; it does not delete `.agent-collab/` or global truth.

When the user explicitly asks to start fresh instead of migrating legacy
state, keep the owners separate:

1. Use `collab migrate` when the project journal is replayable; otherwise use
   the explicitly authorized `collab reset --project --discard-legacy` sequence above
   for Collab-owned state. Do not manually remove `.agent-collab/`.
2. From a clean non-`main` owner worktree, use `appsdk init --fresh
   --discard-legacy` for `.appsdk/` and `.appsdk-control/`. Do not manually
   remove either AppSDK-owned root.
3. Initialize and bind the current project contract, then run `appsdk guide
   compile` and `appsdk verify`. A reset proves only reset; it does not prove
   delivery, review, install, restart, or communication.

## Registration verification

### Where registration and identity queries run

`collab context` is the single agent identity bootstrap. AppSDK project
initialization remains its own owner and may invoke the same daemon context
internally; the agent must not rerun AppSDK initialization or run another
identity command to repair pending Collab. Run `collab context` once from the
project or worktree. Global Collab state resolves the current Codex
sessionID/App Server thread to the canonical route and reports the inherited
identity, liveness, tasks, inbox, `next_actions`, and master/authority state.
`registered: true` ends bootstrap. If the snapshot returns `required_fields`,
supply only those real facts once:

```sh
collab context --provide '<JSON>'
```

The supplement may contain only requested `session_id`, `thread_id`,
`endpoint`, or `namespace` facts; it never supplies a worker, approval, token,
route, or binding. The supplement invocation returns the resulting snapshot.
`collab context` is the registration truth for `authority`, `identity`, `inbox`, `liveness`,
`master`, `next_actions`, `role_brief`, `tasks`, and
`truth`. Registration returns the brief effective at registration; `collab
context` projects the current brief, and promotion or delegation returns the
replacement brief. Read master/authority state from the same snapshot. A live
master exists iff the returned `master` is an object with
`endpoint_live=true`. `master: null` means no live master is recorded; a
`master` object with `endpoint_live=false` is a recorded-but-dead identity and
is not a live master. Do not run a separate master, route, or worker query for
bootstrap. Do not inspect journal, mailbox, `routes.jsonl`, or `~/.collab`
paths to prove registration. Missing or failed identity prevents claiming
registration.

If `collab context` explicitly reports daemon DOWN, a runtime error,
`PROJECT_SCOPE_UNKNOWN`, or `token mismatch`, preserve the exact error and stop
identity repair. Do not infer worktree scope from the error code alone, do not
silently switch to a guessed parent or another project, and do not copy or edit
identity/token state. If the snapshot shows a live master, report the exact
context error to that master. If no live master exists, report it to the
explicitly authorized migration/reset owner or the user. Do not re-register
the worktree, start a daemon, reset the project, or promote a peer. Daemon
lifecycle maintenance is human-authorized.

See [`init-prompts.md`](init-prompts.md) for copy/paste master and peer
initialization prompts.
