# AppSDK + Collab: one query, one binding

Current client: Codex only. The binding is the Codex sessionID.

## State ownership

Global truth:

```text
~/.appsdk/projects.jsonl
~/.appsdk/runtimes.jsonl
~/.appsdk/communication.jsonl
~/.collab/server.sock
~/.collab/events.jsonl
~/.collab/log.txt
```

Project-local `.appsdk/`, `.appsdk-control/`, and `.agent-collab/` are not the
global truth. `.appsdk/` is the committed project contract/maps/records;
`.appsdk-control/` is ignored local run/cache state; `.agent-collab/` is the
project registration/reducer input. Handle them only through
the AppSDK reset or Collab migration/reset owner. Do not inspect or edit them
to decide whether the peer is registered.

## All state

Run one command:

```sh
collab context
```

`collab context` returns identity, liveness, tasks, inbox, `next_actions`,
master/authority state, `role_brief`, and truth. Registration returns the brief
effective at registration; `collab context` and `collab who` project the
current brief, and promotion or delegation returns the replacement brief. That
output is the truth. Stop after reading it. Do not inspect local
environment/control paths or run any other exploratory command after it.
Registration and wake use the internal Codex App Server native thread.

Ordinary initialization and Collab registration run only in the canonical
project main checkout. A Git worktree contains tracked `.appsdk/` files but
does not inherit ignored `.agent-collab/` or `.appsdk-control/` state. The
separate authorized AppSDK `--fresh --discard-legacy` reset may run from its
clean non-main owner worktree as specified below. Inside a worktree, the same
Codex sessionID/thread remains the same peer. Run `collab context` directly
there; it resolves the canonical project root from global Collab state and
reports the inherited identity, liveness, tasks, inbox, `next_actions`, and
master/authority state. Never register the worktree as a second peer, promote
yourself, or create a second route.

## If unregistered

An unregistered project may fail `collab context` instead of returning an
`unregistered` field. Run the idempotent registration once from the project
main tree, then use `collab context` only to verify the resulting identity:

```sh
cd /abs/path/project
appsdk init .
```

Then run `collab context` to verify the registration. Do not run
`appsdk init .` repeatedly; it is idempotent and returns the same
initialization result every time.

## Master (after user approval for the exact project + peer)

```sh
cd /abs/path/project
collab context                  # verify sessionID binding and role
collab master status            # authoritative live-master query
# if the project is unregistered:
appsdk init .
collab context
# only when no live master exists and the user approved this exact peer:
collab master promote --approval "<user approval text>"
collab context
# only after the plan exists and long-horizon work is approved:
appsdk goal subscribe --goal docs/goals/<feature>-plan.md --interval 10m
appsdk goal status --json       # active/observed/collab_subscribed
```

The master then owns orchestration:

1. Query `collab context` once. It must show the identity, liveness, tasks,
   inbox, `next_actions`, `role_brief`, and master/authority state.
2. Split the confirmed goal by dependency and unique write scope. Dispatch
   through `collab subagent dispatch` or `appsdk subagent send`; every
   assignment needs done-iff, artifacts, forbidden paths, exact tests, and
   evidence location.
3. Before execution or dispatch, run `appsdk bug intake --input <json>` and
   bind the returned `issue_id`; read-only conversation skips this path.
4. Keep workers saturated from the approved task graph, then from
   `appsdk bug list --status open --json` in `P0 > P1 > P2` order.
5. Own blockers, re-dispatch or auditable force-close stuck tasks, integrate
   reviewed commits on latest main, and keep source/review/merge/install/
   restart/live-replay evidence separate.
6. Remove only resources created by this round. Preserve other peers'
   worktrees, processes, and evidence.

Stop normal setup here. No `collab status --all`, no `routes.jsonl`, no
`whoami`, no `ps`, no `.agent-collab` listing. The explicit stale-daemon
recovery procedure below is the only exception: it may run `collab status
--all` to diagnose the unavailable host daemon.

## Long-horizon master initialization and timer proof

The master creates the plan file before registering the timer:

```sh
appsdk goal subscribe --goal docs/goals/<feature>-plan.md --interval 10m
appsdk goal status --json
appsdk longhorizon show --json
```

`appsdk goal status --json` must report `active: true`,
`desired: subscribed`, `observed: subscribed`, `collab_subscribed: true`, a
non-null `subscription_id`, and `error: null`. Command output alone is not
timer proof. Run one short-interval live replay and record the armed
subscription, fired deadline notification, and consumed result. The current
implementation is a one-shot deadline; rearm it after consumption, expiry, or
a Collab restart.

## Upstream AppSDK bug report

When a project hits a defect in AppSDK itself, do not patch around it or hide
it in project-local state:

```sh
appsdk bug list -q "<symptom>" --json --upstream
appsdk bug new --upstream -t "[SDK Bug] <symptom>" -m "<reproduction, expected, observed, version, commit, logs>" -l "P0,appsdk"
appsdk bug show <id> --json --upstream
```

Include the source commit, binary version/hash, exact command, first failing
layer, and whether the same path fails from a clean project. The upstream bug
is a report and evidence record; it is not proof that the local delivery
passed.

`--upstream` is the explicit git-bug upstream route. The report must use the
actual symptom, reproduction, expected/observed result, version/commit,
and relevant logs; do not turn it into a local project bug or a fallback
workaround.

## Ordinary peer (project already has .appsdk/project.json and a live master)

```sh
cd /abs/path/project
collab context
```

If the project is unregistered, run `appsdk init .` once and then `collab
context` to verify the binding. If it reports `role=master`, stop and report
the conflict to the master; do not promote yourself and do not start a second
daemon.

Use `collab master status` for the live master. A live master exists iff the
returned `master` is an object with `endpoint_live=true`. `master: null` means
no live master is recorded; a `master` object with `endpoint_live=false` is a
recorded-but-dead identity and is not a live master. `collab who` only lists
registered peers and does not contain a top-level `master` field. A worktree
normally has no local `.agent-collab/`; that does not mean the peer is
unregistered or that no master exists. A failed `collab context`, including
`token mismatch`, is a registration problem, not evidence of no master. If it
fails, preserve the exact error and query `collab master status` separately:
report the registration error to the live master only when `endpoint_live=true`;
when no live master exists, report it to the explicitly authorized
migration/reset owner or the user and stop registration repair. Do not infer
"no master", copy/edit identity state, reset, or promote yourself from the
worktree.

For an explicitly authorized clean epoch, the reset owners are separate. The
AppSDK line is a reset/reinitialize operation, not ordinary initialization:

```sh
# AppSDK-owned project control plane, from a clean non-main owner worktree
appsdk init <project> --fresh --discard-legacy

# Collab-owned project control plane, during a controlled maintenance window
collab down
collab reset --discard-legacy --approval "<user authorization>"
collab up
collab init
```

Neither reset removes the other owner's state or proves delivery, review,
install, restart, or live communication.

## Recover own binding

If `collab context` reports a missing binding with no token mismatch:

```sh
collab worker recover
```

Then run `collab context` again. Do not edit `~/.collab`, do not grep
`routes.jsonl`, do not touch `server.pid`, do not inspect terminal environment
paths, do not start a second daemon.

If recovery returns `token mismatch`, stop. That error means the global
identity token does not own the project reducer's registered worker; it is not
repairable by copying the token or editing identity files. Preserve the exact
error and query `collab master status` separately. Report the registration
problem to the live master only when `endpoint_live=true`; otherwise report it
to the explicitly authorized migration/reset owner or the user and stop
registration repair. Use the migration/reset owner only when that owner
explicitly decides the project-local control plane is unrecoverable.

If recovery reports `DAEMON_UNAVAILABLE` or a stale route, use the controlled
lifecycle first:

```sh
collab down
collab up
collab context
```

If `collab up` reports `HOST_ROUTE_REPLAY_FAILED` for a named missing root,
preserve the exact output and follow the installed Collab migration reference.
Never delete the route file or project state by hand.

## Stale daemon, socket, or lock

`~/.collab/server.sock`, `server.pid`, and `daemon.lock` are host-owned runtime
objects. A stale socket/PID after a crash is not permission to remove them by
hand. Diagnose in this order from the canonical project root:

```sh
collab context
collab status --all
collab up
collab context
```

If `collab context` reports an unavailable daemon, use `collab up` once and
preserve its exact output. If `collab up` reports a stale lock, a second
writer, or an unknown PID, stop and report the exact error plus the canonical
project root; the Collab owner must repair the global daemon. Never use
`pkill`, `killall`, `kill $(...)`, delete `daemon.lock`, unlink `server.sock`,
or start a project-local daemon as a workaround.
