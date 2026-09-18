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
`.appsdk-control/` is ignored local run/cache state; `.agent-collab/` is
Collab-owned project reducer/journal/mailbox state. Handle them only through
the AppSDK reset or Collab migration/reset owner. Do not inspect or edit them
to decide whether the peer is registered.

## All state

Run one command:

```sh
collab context
```

`collab context` returns your Codex sessionID binding, role, identity,
transport, liveness, current project state, and peers. That output is the
truth. Stop after reading it. Do not inspect local environment/control paths
or run any other exploratory command after it. Registration and wake use the
internal Codex App Server native thread.

## If unregistered

`collab context` will tell you that you are not registered. Run the
idempotent registration once, from the project main tree:

```sh
cd /abs/path/project
appsdk init .
```

Then run `collab context` again. Do not run `appsdk init .` repeatedly;
it is idempotent and returns the same initialization result every time.

## Master (after user approval for the exact project + peer)

```sh
cd /abs/path/project
collab context                  # verify sessionID binding and role
# only if collab context says unregistered:
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

1. Query `collab context` once. It must show the Codex sessionID binding,
   `role=master`, App Server transport, liveness, project state, and peers.
2. Split the confirmed goal by dependency and unique write scope. Dispatch
   through `collab subagent dispatch` or `appsdk subagent send`; every
   assignment needs done-iff, artifacts, forbidden paths, exact tests, and
   evidence location.
3. Keep workers saturated from the approved task graph, then from
   `appsdk bug list --status open --json` in `P0 > P1 > P2` order.
4. Own blockers, re-dispatch or auditable force-close stuck tasks, integrate
   reviewed commits on latest main, and keep source/review/merge/install/
   restart/live-replay evidence separate.
5. Remove only resources created by this round. Preserve other peers'
   worktrees, processes, and evidence.

Stop setup here. No `collab status --all`, no `routes.jsonl`, no `whoami`, no
`ps`, no `.agent-collab` listing.

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
appsdk bug list -q "<symptom>" --json
appsdk bug new --upstream -t "[SDK Bug] <symptom>" -m "<reproduction, expected, observed, version, commit, logs>" -l "P0,appsdk"
appsdk bug show <id> --json
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

If `collab context` says unregistered, run `appsdk init .` once and then
`collab context` again. If it reports `role=master`, stop and report the
conflict to the master; do not promote yourself and do not start a second
daemon.

For an explicitly authorized clean epoch, the reset owners are separate:

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

If `collab context` reports the wrong or missing binding:

```sh
collab worker recover
```

Then run `collab context` again. Do not edit `~/.collab`, do not grep
`routes.jsonl`, do not touch `server.pid`, do not inspect terminal environment
paths, do not start a second daemon.

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
