# AppSDK + Collab Initialization

One command. Read its self-check line. Stop.

```sh
cd /abs/path/project           # main tree, not a worktree
appsdk init .                  # self-check IS the registration result
```

`appsdk init .` prints a single line with `worker_id`, `role`,
`transport.kind`, `identity_valid`, `endpoint_live` and the
`direct-message` subscription. That line is the truth. Do not run `collab
who`, `collab context`, `whoami`, `routes.jsonl` grep, `pane` probing,
`ps`, `find .agent-collab`, `appsdk present`, `appsdk context`, or any
other exploratory command as part of initialization. Probing is a
recovery action, not a setup action.

## Master (after user approval for the exact project + peer)

```sh
cd /abs/path/project
appsdk init .                                          # self-check = registration
collab master promote --approval "<user approval text>" # only after self-check OK
appsdk goal subscribe --goal docs/goals/<feature>-plan.md --interval 10m
appsdk goal status --json                               # active/observed/collab_subscribed
```

Stop here. The four lines above are the entire master bootstrap. No
`collab status --all`, no `routes.jsonl` checks, no `whoami`, no `ps`.

## Ordinary peer (project already has .appsdk/project.json and a live master)

```sh
cd /abs/path/project
appsdk init .                  # self-check = registration, role must be peer
```

Stop here. If the self-check reports `role=master`, stop and report the
conflict to the master; do not promote yourself and do not start a second
daemon.

## Recovery (only when self-check reports an error)

If `appsdk init .` returns `error: cannot resolve tmux session for pane
%<n>` or `RUNTIME_BINDING_REJECTED: pane ownership for %<n> is unknown`,
the shell is not bound to the registered pane/thread. Run the same
command from that pane/thread, or run `collab worker recover` inside a
fresh live pane and re-bind the same `worker_id`. No file grepping,
no `ps`, no `routes.jsonl` editing.

If it returns `HOST_ROUTE_REPLAY_FAILED: canonical root <path>: No such
file or directory`, run the documented stale route cleanup once:

```sh
collab down
cp ~/.collab/routes.jsonl ~/.collab/routes.jsonl.before-stale-cleanup-$(date +%Y%m%d-%H%M%S)
grep -v '<missing root>' ~/.collab/routes.jsonl > ~/.collab/routes.jsonl.tmp
mv ~/.collab/routes.jsonl.tmp ~/.collab/routes.jsonl
collab up
cd /abs/path/project && appsdk init .
```

## Where registration must run

Always from the project main tree. The daemon refuses worktree paths with
`must run from the project main tree`. If you are in
`playground/<slug>`, `cd` back to the canonical root registered in
`~/.collab/routes.jsonl` before running `appsdk init .`.
