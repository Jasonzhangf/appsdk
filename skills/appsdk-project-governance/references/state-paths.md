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
appsdk reset-governance --discard-legacy
                               -> AppSDK control-plane reset
appsdk init --fresh --discard-legacy
                               -> preferred single transaction for old AppSDK control plane
collab down
collab reset --discard-legacy --approval "<user text>"
collab up
collab init
                               -> Collab-owned project control-plane reset
```

For old `.appsdk/` state, do not delete it manually. Use the authorized reset
route after the Collab side is migrated or retired. `appsdk init --fresh
--discard-legacy` removes the AppSDK-owned old control plane and rebuilds the
current baseline; it does not delete `.agent-collab/` or global truth.

When the user explicitly asks to start fresh instead of migrating legacy
state, keep the owners separate:

1. Use `collab migrate` when the project journal is replayable; otherwise use
   the explicitly authorized `collab reset --discard-legacy` sequence above
   for Collab-owned state. Do not manually remove `.agent-collab/`.
2. From a clean non-`main` owner worktree, use `appsdk init --fresh
   --discard-legacy` for `.appsdk/` and `.appsdk-control/`. Do not manually
   remove either AppSDK-owned root.
3. Initialize and bind the current project contract, then run `appsdk guide
   compile` and `appsdk verify`. A reset proves only reset; it does not prove
   delivery, review, install, restart, or communication.

## Registration verification

### Where registration and identity queries run

`appsdk init` / `collab init` registers the **canonical project root** — the
main checkout of the project, not a worktree. Run those initialization
commands only from the canonical root. A worktree does not need a second
registration: run `collab context` directly there. Global Collab state
resolves the current Codex sessionID/App Server thread to the canonical route
and reports the inherited identity, liveness, tasks, inbox, `next_actions`,
and master/authority state.

Use `collab master status` as the authoritative live-master query. A live
master exists iff the returned `master` is an object with
`endpoint_live=true`. `master: null` means no live master is recorded; a
`master` object with `endpoint_live=false` is a recorded-but-dead identity and
is not a live master. `collab who` only lists registered peers and has no
top-level `master` field. Never infer "no master" from a missing
worktree-local `.agent-collab/`, a failed `collab context`, a `token mismatch`,
or `collab who` output. Do not register the worktree as a new peer, promote
yourself, create a second route, or edit `routes.jsonl`.

After `collab init`, do not stop at command success. Verify with the one
authoritative query first:

```sh
collab context
```

`collab context` is the registration truth for `authority`, `identity`,
`inbox`, `liveness`, `master`, `next_actions`, `tasks`, and `truth`. It does
not return `role_brief` or `peers`; `role_brief` comes only from the
registration receipt, and `collab who` is the peer-list command. `collab
master status` is the separate live-master truth. Do not inspect journal,
mailbox, `routes.jsonl`, or `~/.collab` paths to prove registration. Missing
or failed identity prevents claiming registration.

If `collab context` fails with `PROJECT_SCOPE_UNKNOWN` or `token mismatch`,
preserve the exact error and stop registration repair. Do not infer worktree
scope from the error code alone, do not silently switch to a guessed parent or
another project, and do not copy or edit identity/token state. Check `collab
master status` separately. If `endpoint_live=true`, report the exact context
error to that live master. If no live master exists, report it to the
explicitly authorized migration/reset owner or the user. Do not re-register
the worktree, start a daemon, reset the project, or promote a peer. Use the
migration/reset owner only when that owner explicitly decides the
project-local control plane is unrecoverable.

See [`init-prompts.md`](init-prompts.md) for copy/paste master and peer
initialization prompts.
