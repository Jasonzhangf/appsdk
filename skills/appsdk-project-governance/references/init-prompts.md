# AppSDK + Collab: one query, one binding

Current client: Codex only. The binding is the Codex sessionID.

## All state

Run one command:

```sh
collab context
```

`collab context` returns your Codex sessionID binding, role, identity,
transport, liveness, current project state, and peers. That output is the
truth. Stop after reading it. Do not run `collab who`, `collab status
--all`, `whoami`, `routes.jsonl` grep, `pane` probing, `ps`, `find
.agent-collab`, `appsdk present`, or any other exploratory command after
it.

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
appsdk init .                   # only if collab context says unregistered
collab master promote --approval "<user approval text>"
appsdk goal subscribe --goal docs/goals/<feature>-plan.md --interval 10m
appsdk goal status --json       # active/observed/collab_subscribed
```

Stop here. No `collab status --all`, no `routes.jsonl`, no `whoami`, no
`ps`, no `.agent-collab` listing.

## Ordinary peer (project already has .appsdk/project.json and a live master)

```sh
cd /abs/path/project
collab context
```

If `collab context` says unregistered, run `appsdk init .` once and then
`collab context` again. If it reports `role=master`, stop and report the
conflict to the master; do not promote yourself and do not start a second
daemon.

## Recover own binding

If `collab context` reports the wrong or missing binding:

```sh
collab worker recover
```

Then run `collab context` again. Do not edit `~/.collab`, do not grep
`routes.jsonl`, do not touch `server.pid`, do not start a second daemon.
