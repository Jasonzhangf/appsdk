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
effective at registration; `collab context` projects the current brief, and
promotion or delegation returns the replacement brief. That output is the
truth. `registered: true` ends bootstrap. If the snapshot returns
`required_fields`, supply only those real facts once:

```sh
collab context --provide '<JSON>'
```

The supplement may contain only requested `session_id`, `thread_id`,
`endpoint`, or `namespace` facts; it never supplies a worker, approval, token,
route, or binding. The supplement invocation returns the resulting snapshot.
Do not choose a worker or run another identity command. If context returns an
explicit daemon DOWN or runtime error, preserve the exact error and stop;
daemon lifecycle maintenance is human-authorized. Do not inspect local
environment/control paths or run any other exploratory command. Registration
and wake use the internal Codex App Server native thread.

Ordinary AppSDK project initialization runs only in the canonical project main
checkout. Agent identity bootstrap is `collab context`; it may run from the
project or worktree, and the daemon resolves the canonical route. A Git
worktree contains tracked `.appsdk/` files but does not inherit ignored
`.agent-collab/` or `.appsdk-control/` state. The separate authorized AppSDK
`--fresh --discard-legacy` reset may run from its clean non-main owner worktree
as specified below. Inside a worktree, the same Codex sessionID/thread remains
the same peer. Run `collab context` directly there; it reports the inherited
identity, liveness, tasks, inbox, `next_actions`, and master/authority state.
Never register the worktree as a second peer, promote yourself, or create a
second route.

## If required facts are missing

`collab context` can return `registered: false`, `identity: null`, and
`required_fields`. Supply only the real requested facts once with
`collab context --provide '<JSON>'`. The supplement may contain only requested
`session_id`, `thread_id`, `endpoint`, or `namespace` facts; it never supplies
a worker, approval, token, route, or binding. The daemon owns identity
creation, selection, restoration, update, registration, route publication, and
lease restoration. Do not run AppSDK or Collab initialization to repair
identity, and do not inspect routes or worker state to guess a binding.

## Master (after user approval for the exact project + peer)

```sh
cd /abs/path/project
collab context                  # verify sessionID binding and role
# if required_fields are present, provide only those facts once
# only when the context snapshot has no live master and the user approved
# this exact peer:
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

Stop normal setup here. Do not run operator diagnostics, read `routes.jsonl`,
inspect processes, or list `.agent-collab/`. If context explicitly reports
daemon DOWN or a runtime error, preserve the exact error and stop; daemon
lifecycle maintenance is human-authorized.

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
# if required_fields are present, provide only those facts once
```

If context reports `role=master`, stop and report the conflict to the master;
do not promote yourself and do not start a second daemon.

Read master/authority state from the same context snapshot. A live master
exists iff the returned `master` is an object with `endpoint_live=true`.
`master: null` means no live master is recorded; a `master` object with
`endpoint_live=false` is a recorded-but-dead identity and is not a live master.
A worktree normally has no local `.agent-collab/`; that does not mean the peer
is unregistered or that no master exists. A failed `collab context`, including
`token mismatch`, is a registration problem, not evidence of no master. If it
fails, preserve the exact error. Report the registration error to the live
master only when the snapshot shows `endpoint_live=true`; when no live master
exists, report it to the explicitly authorized migration/reset owner or the
user and stop identity repair. Do not infer "no master", copy/edit identity
state, reset, or promote yourself from the worktree.

For an explicitly authorized clean epoch, the reset owners are separate. The
AppSDK line is a reset/reinitialize operation, not ordinary initialization:

```sh
# AppSDK-owned project control plane, from a clean non-main owner worktree
appsdk init <project> --fresh --discard-legacy

# Collab-owned project control plane, during a controlled maintenance window
collab down
collab reset --discard-legacy --approval "<user authorization>"
collab up
collab context
```

Neither reset removes the other owner's state or proves delivery, review,
install, restart, or live communication.

## Identity and route reconciliation

Identity creation, selection, restoration, update, route publication,
registration, and lease restoration belong to the daemon. `collab context` and
its one factual supplement are the only agent bootstrap. Do not copy tokens,
edit identity state, or run separate route/status discovery for repair. If
context explicitly reports daemon DOWN or a runtime error, preserve the exact
error and stop.

## Stale daemon, socket, or lock

`~/.collab/server.sock`, `server.pid`, and `daemon.lock` are host-owned runtime
objects. If `collab context` explicitly reports daemon DOWN or a runtime error,
preserve the exact output and stop. Starting, stopping, or restarting the host
daemon is human-authorized maintenance. An authorized human operator may use
the official `collab down` / `collab up` lifecycle when a maintenance window is
approved. Never diagnose identity by chaining status commands, remove lock or
socket files by hand, use broad process-kill commands, or start a project-local
daemon.
